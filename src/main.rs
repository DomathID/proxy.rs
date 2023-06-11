#![allow(dead_code)]
//#![allow(unused_variables)]
#![allow(unused_imports)]
//#![allow(unreachable_code)]

use argument::{FindArgs, GrabArgs};
use checker::Checker;
use clap::Parser;
use lazy_static::lazy_static;
use parking_lot::Mutex;
use proxy::Proxy;
use regex::Regex;
use simple_logger::SimpleLogger;
use std::{path::PathBuf, pin::Pin, process, sync::Arc, time::Duration};
use tokio::{
    fs::File,
    io::{stdout, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    runtime,
    sync::mpsc::{self, UnboundedSender},
    task, time,
};

use crate::{
    argument::{Cli, Commands},
    providers::PROXIES,
    utils::run_parallel,
};

mod argument;
mod checker;
mod judge;
mod negotiators;
mod providers;
mod proxy;
mod resolver;
mod utils;

lazy_static! {
    static ref STOP_FIND_LOOP: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
}

const EOF_MSG: &str = "==EOF==";

async fn handle_grab_command(args: GrabArgs, tx: UnboundedSender<String>) {
    let format = args.format;
    let limit = args.limit;
    let mut counter = 1;
    let mut stop = false;

    if format == "json" {
        tx.send("[".to_string()).unwrap();
    }

    while !stop {
        while let Ok((host, port, expected_types)) = PROXIES.pop() {
            if stop {
                break;
            }
            if let Some(proxy) = proxy::Proxy::create(host.as_str(), port, expected_types).await {
                counter += 1;
                let mut msg = String::new();
                msg.push_str(
                    match format.as_str() {
                        "text" => proxy.as_text(),
                        "json" => proxy.as_json(),
                        _ => format!("{}", proxy),
                    }
                    .as_str(),
                );

                if limit != 0 && counter > limit {
                    msg.push_str(if format == "json" { "]" } else { "" });
                    stop = true;
                } else if format == "json" {
                    msg.push(',')
                }
                msg.push('\n');
                tx.send(msg).unwrap()
            }
        }
    }
    tx.send(EOF_MSG.to_string()).unwrap();
}

async fn handle_find_command(
    mut checker: Checker,
    args: FindArgs,
    max_conn: usize,
    tx: UnboundedSender<String>,
) {
    // config
    let format = args.format;
    let limit = args.limit;

    checker.expected_types = args.types;
    checker.expected_levels = args.levels;
    checker.expected_countries = args.countries;

    let counter = Arc::new(Mutex::new(1));

    if format == "json" {
        tx.send("[".to_string()).unwrap();
    }

    while !*STOP_FIND_LOOP.lock() {
        let mut proxies = Vec::with_capacity(5000);
        while let Ok((host, port, expected_types)) = PROXIES.pop() {
            if proxies.len() >= 5000 {
                break;
            }
            if let Some(mut proxy) = Proxy::create(host.as_str(), port, expected_types).await {
                let mut checker_clone = checker.clone();
                let counter = counter.clone();
                let limit = limit;
                let format = format.clone();
                let tx = tx.clone();
                proxies.push(task::spawn(async move {
                    if checker_clone.check_proxy(&mut proxy).await {
                        let mut counter = counter.lock();
                        *counter += 1;

                        let mut msg = String::new();
                        msg.push_str(
                            match format.as_str() {
                                "text" => proxy.as_text(),
                                "json" => proxy.as_json(),
                                _ => format!("{}", proxy),
                            }
                            .as_str(),
                        );

                        let end = limit != 0 && *counter > limit;
                        if end {
                            msg.push_str(if format == "json" { "]" } else { "" });
                            let mut stop_file_loop = STOP_FIND_LOOP.lock();
                            *stop_file_loop = true;
                        } else if format == "json" {
                            msg.push(',');
                        }
                        msg.push('\n');
                        tx.send(msg).unwrap();
                        if end {
                            tx.send(EOF_MSG.to_string()).unwrap()
                        }
                    }
                }));
            }
        }

        if !proxies.is_empty() {
            let stime = time::Instant::now();
            let ret = run_parallel::<()>(proxies, Some(max_conn)).await;
            log::info!(
                "Finished checking {} proxies. Runtime {:?}",
                ret.len(),
                stime.elapsed()
            )
        }
    }
}

async fn handle_file_input(files: Vec<PathBuf>) {
    for file in files {
        match File::open(&file).await {
            Ok(file) => {
                let ip_port = Regex::new(r#"(?P<ip>(?:\d+\.?){4}):(?P<port>\d+)"#).unwrap();
                let buffer = BufReader::new(file);
                let mut lines = buffer.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(cap) = ip_port.captures(&line) {
                        let ip = cap.get(1).unwrap().as_str();
                        let port = cap.get(2).unwrap().as_str();

                        if let Ok(port) = port.parse::<u16>() {
                            PROXIES.push((ip.to_string(), port, vec![])).unwrap();
                        };
                    }
                }
            }
            Err(e) => log::error!("{}: {:?}", e, file),
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let log_level = match cli.log_level.as_str() {
        "debug" => log::LevelFilter::Debug,
        "info" => log::LevelFilter::Info,
        "error" => log::LevelFilter::Error,
        _ => log::LevelFilter::Warn,
    };

    SimpleLogger::new()
        .with_level(log::LevelFilter::Off)
        .with_module_level("proxy_rs", log_level)
        .without_timestamps()
        .init()
        .unwrap();

    log::info!("Start collecting proxies.. ");

    runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let mut tasks = vec![];

            let max_tries = cli.max_tries as i32;
            let max_conn = cli.max_conn;
            let timeout = cli.timeout as i32;

            let mut files = vec![];
            let (tx, mut rx) = mpsc::unbounded_channel();
            let outfile;

            match cli.sub {
                Commands::Grab(grab_args) => {
                    outfile = grab_args.outfile.clone();

                    let tx = tx.clone();
                    tasks.push(task::spawn(handle_grab_command(grab_args, tx)))
                }
                Commands::Find(find_args) => {
                    outfile = find_args.outfile.clone();

                    let mut checker = Checker::new().await;
                    checker.max_tries = max_tries;
                    checker.timeout = timeout;
                    checker.support_cookie = find_args.support_cookies;
                    checker.support_referer = find_args.support_referer;

                    let ext_ip = checker.ext_ip.clone();

                    let expected_types = find_args.types.clone();
                    let verify_ssl = false;
                    tasks.push(task::spawn(async move {
                        checker::check_judges(verify_ssl, ext_ip, expected_types).await;
                    }));

                    files.extend(find_args.files.clone());

                    let tx = tx.clone();
                    tasks.push(task::spawn(handle_find_command(
                        checker, find_args, max_conn, tx,
                    )));
                }
            }

            if !files.is_empty() {
                tasks.push(task::spawn(async move {
                    handle_file_input(files).await;
                    let mut stop_file_loop = STOP_FIND_LOOP.lock();
                    *stop_file_loop = true
                }))
            } else {
                /* providers */
                tasks.push(tokio::task::spawn(async {
                    loop {
                        let all_providers = providers::get_all_tasks();
                        run_parallel::<()>(all_providers, None).await;
                        time::sleep(Duration::from_secs(10)).await;
                    }
                }));
            }

            let mut output: Pin<Box<dyn AsyncWrite>> = if let Some(path) = outfile {
                let file = File::create(path).await.unwrap();
                Box::pin(file)
            } else {
                Box::pin(stdout())
            };

            while let Some(msg) = rx.recv().await {
                if msg == EOF_MSG {
                    break;
                }
                output.write_all(msg.as_bytes()).await.unwrap();
            }
            std::process::exit(0);
        });
}
