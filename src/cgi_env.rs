//! Simple CGI environment utilities

use std::collections::HashMap;

/// Simple CGI environment structure
pub struct CgiEnv {
    pub query_string: String,
    pub request_method: String,
    pub content_length: Option<usize>,
    pub content_type: Option<String>,
    pub headers: HashMap<String, String>,
}

impl CgiEnv {
    pub fn new() -> Self {
        Self {
            query_string: String::new(),
            request_method: String::new(),
            content_length: None,
            content_type: None,
            headers: HashMap::new(),
        }
    }

    pub fn from_request(method: &str, uri: &str, host: &str, query_string: &str, headers: &HashMap<String, String>) -> Self {
        let mut env = Self::new();
        env.request_method = method.to_string();
        env.query_string = query_string.to_string();
        env.headers = headers.clone();
        env
    }

    pub fn parse_query(&self) -> HashMap<String, String> {
        let mut params = HashMap::new();
        for pair in self.query_string.split('&') {
            if let Some(eq_pos) = pair.find('=') {
                let key = url_decode(&pair[..eq_pos]);
                let value = url_decode(&pair[eq_pos + 1..]);
                params.insert(key, value);
            }
        }
        params
    }
}

/// URL decode a string
pub fn url_decode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    
    while let Some(ch) = chars.next() {
        if ch == '+' {
            result.push(' ');
        } else if ch == '%' {
            if let (Some(c1), Some(c2)) = (chars.next(), chars.next()) {
                let hex = format!("{}{}", c1, c2);
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(char::from(byte));
                } else {
                    result.push('%');
                    result.push(c1);
                    result.push(c2);
                }
            } else {
                result.push('%');
            }
        } else {
            result.push(ch);
        }
    }
    
    result
}
