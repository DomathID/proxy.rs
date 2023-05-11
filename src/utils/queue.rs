use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

#[derive(Debug, Clone)]
pub struct FifoQueue<T> {
    pub data: Arc<Mutex<VecDeque<T>>>,
    cv: Arc<Condvar>,
}

impl<T: std::cmp::PartialEq> FifoQueue<T> {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(VecDeque::new())),
            cv: Arc::new(Condvar::new()),
        }
    }

    pub fn push(&self, value: T) {
        let mut data = self.data.lock().unwrap();
        data.push_back(value);
        self.cv.notify_one();
    }

    pub fn push_unique(&self, value: T) -> bool {
        if self.is_unique(&value) {
            self.push(value);
            return true;
        }
        false
    }

    pub fn get(&self) -> T {
        let mut data = self.data.lock().unwrap();
        while data.is_empty() {
            data = self.cv.wait(data).unwrap();
        }
        data.pop_front().unwrap()
    }

    pub fn get_nowait(&self) -> Option<T> {
        let mut data = self.data.lock().unwrap();
        if !data.is_empty() {
            return Some(data.pop_front().unwrap());
        }
        None
    }

    pub fn qsize(&self) -> usize {
        let data = self.data.lock().unwrap();
        data.len()
    }

    pub fn is_empty(&self) -> bool {
        let data = self.data.lock().unwrap();
        data.is_empty()
    }

    pub fn is_unique(&self, value: &T) -> bool {
        let data = self.data.lock().unwrap();
        !data.contains(value)
    }
}

impl<T: std::cmp::PartialEq> std::fmt::Display for FifoQueue<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<FifoQueue {} items>", self.qsize())
    }
}
