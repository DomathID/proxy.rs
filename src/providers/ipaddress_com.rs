use super::base_provider::BaseProvider;
use crate::utils::vec_of_strings;

#[derive(Debug, Clone)]
pub struct IpaddressComProvider {
    pub base: BaseProvider,
    pub url: String,
    pub pattern: String,
}

impl IpaddressComProvider {
    pub async fn get_proxies(&mut self) -> Vec<(String, u16, Vec<String>)> {
        let req = self.base.client.get(self.url.clone());
        let html = self.base.get_html(req).await;
        let proxies = self.base.find_proxies(self.pattern.clone(), html.as_str());
        self.base.update_stack(&proxies).await;

        proxies
    }
}

impl Default for IpaddressComProvider {
    fn default() -> Self {
        Self {
            base: BaseProvider {
                proto: vec_of_strings!["HTTP", "CONNECT:80", "HTTPS", "CONNECT:25"],
                domain: "ipaddress.com".to_string(),
                ..Default::default()
            },
            url: "https://www.ipaddress.com/proxy-list".to_string(),
            pattern: r#"(?P<ip>(?:\d+\.?){4})</a>\:(?P<port>\d+)"#.to_string(),
        }
    }
}
