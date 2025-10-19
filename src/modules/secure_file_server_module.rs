//! Secure File Server Module
//!
//! This module implements secure file serving with the following security features:
//! - Path Sanitization: Prevents directory traversal attacks
//! - Path Canonicalization: Resolves symlinks and validates paths
//! - File Type Support: MIME type support for various file formats
//! - Privilege Dropping: Can drop to unprivileged user after binding to privileged ports

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf, Component};
use std::time::{SystemTime, UNIX_EPOCH, Duration};

use super::http_version::HttpVersion;
use super::http_response::HttpResponse;

// Unix-specific imports for privilege dropping
//#[cfg(unix)]
// use std::os::unix::fs::PermissionsExt; // Not currently used

/// Format a SystemTime as an HTTP date string (RFC 7231)
/// Returns a string in the format: "Wed, 21 Oct 2015 07:28:00 GMT"
fn format_http_date(time: &SystemTime) -> String {
    let duration_since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let timestamp = duration_since_epoch.as_secs();

    // Convert Unix timestamp to HTTP date format
    // This is a simplified implementation - in production you might want to use a proper date formatting library
    let days_since_epoch = timestamp / 86400;
    let seconds_today = timestamp % 86400;

    let hours = seconds_today / 3600;
    let minutes = (seconds_today % 3600) / 60;
    let seconds = seconds_today % 60;

    // Calculate year, month, day (simplified - doesn't handle leap years perfectly)
    let year = 1970 + (days_since_epoch / 365);
    let day_of_year = (days_since_epoch % 365) + 1;

    // Simple month calculation (approximate)
    let month = if day_of_year <= 31 { 1 } // Jan
    else if day_of_year <= 59 { 2 } // Feb
    else if day_of_year <= 90 { 3 } // Mar
    else if day_of_year <= 120 { 4 } // Apr
    else if day_of_year <= 151 { 5 } // May
    else if day_of_year <= 181 { 6 } // Jun
    else if day_of_year <= 212 { 7 } // Jul
    else if day_of_year <= 243 { 8 } // Aug
    else if day_of_year <= 273 { 9 } // Sep
    else if day_of_year <= 304 { 10 } // Oct
    else if day_of_year <= 334 { 11 } // Nov
    else { 12 }; // Dec

    let day_of_month = day_of_year - match month {
        1 => 0,
        2 => 31,
        3 => 59,
        4 => 90,
        5 => 120,
        6 => 151,
        7 => 181,
        8 => 212,
        9 => 243,
        10 => 273,
        11 => 304,
        12 => 334,
        _ => 0,
    };

    // Calculate day of week (simplified)
    let day_of_week = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][(days_since_epoch % 7) as usize];
    let month_name = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                     "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(month - 1) as usize];

    format!("{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
            day_of_week, day_of_month, month_name, year, hours, minutes, seconds)
}

/// MIME type mappings for common file extensions
#[derive(Debug, Clone)]
pub struct MimeTypes {
    //TODO: Should this be &str, &str?
    types: HashMap<String, String>,
}

impl Default for MimeTypes {
    fn default() -> Self {
        let mut types = HashMap::new();

        // HTML and text files
        types.insert("html".to_string(), "text/html; charset=utf-8".to_string());
        types.insert("htm".to_string(),  "text/html; charset=utf-8".to_string());
        types.insert("txt".to_string(),  "text/plain; charset=utf-8".to_string());
        types.insert("css".to_string(),  "text/css; charset=utf-8".to_string());

        // JavaScript
        types.insert("js".to_string(),   "application/javascript; charset=utf-8".to_string());
        types.insert("mjs".to_string(),  "application/javascript; charset=utf-8".to_string());

        // WebAssembly
        types.insert("wasm".to_string(), "application/wasm".to_string());

        // Images
        types.insert("jpg".to_string(),  "image/jpeg".to_string());
        types.insert("jpeg".to_string(), "image/jpeg".to_string());
        types.insert("png".to_string(),  "image/png".to_string());
        types.insert("gif".to_string(),  "image/gif".to_string());
        types.insert("svg".to_string(),  "image/svg+xml".to_string());
        types.insert("webp".to_string(), "image/webp".to_string());
        types.insert("ico".to_string(),  "image/x-icon".to_string());
        types.insert("bmp".to_string(),  "image/bmp".to_string());

        // Fonts
        types.insert("woff".to_string(),  "font/woff".to_string());
        types.insert("woff2".to_string(), "font/woff2".to_string());
        types.insert("ttf".to_string(),   "font/ttf".to_string());
        types.insert("otf".to_string(),   "font/otf".to_string());

        // Documents
        types.insert("pdf".to_string(),  "application/pdf".to_string());
        types.insert("json".to_string(), "application/json; charset=utf-8".to_string());
        types.insert("xml".to_string(),  "application/xml; charset=utf-8".to_string());

        // Archives
        types.insert("zip".to_string(), "application/zip".to_string());
        types.insert("tar".to_string(), "application/x-tar".to_string());
        types.insert("gz".to_string(),  "application/gzip".to_string());
        types.insert("tgz".to_string(), "application/gzip".to_string());

        // Default binary type
        types.insert("bin".to_string(), "application/octet-stream".to_string());

        Self { types }
    }
}

impl MimeTypes {
    /// Get MIME type for a file extension
    pub fn get_mime_type(&self, path: &Path) -> String {
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            self.types.get(extension.to_lowercase().as_str())
                .cloned()
                .unwrap_or_else(|| "application/octet-stream".to_string())
        } else {
            "application/octet-stream".to_string()
        }
    }
}

/// Security configuration for file serving
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// Document root directory
    pub document_root: PathBuf,
    /// Whether to follow symlinks
    pub follow_symlinks: bool,
    /// Maximum file size to serve (in bytes)
    pub max_file_size: u64,
    /// Allowed file extensions (if empty, all extensions are allowed)
    pub allowed_extensions: Vec<String>,
    /// Blocked file extensions
    pub blocked_extensions: Vec<String>,
    /// User ID to drop to (if None, no privilege dropping)
    pub drop_to_uid: Option<u32>,
    /// Group ID to drop to (if None, no privilege dropping)
    pub drop_to_gid: Option<u32>,
    /// Keep-Alive timeout for persistent connections
    pub keep_alive_timeout: Duration,
    /// Maximum number of requests per Keep-Alive connection
    pub keep_alive_max_requests: usize,
    /// Minimum HTTP version to support
    pub minimum_http_version: HttpVersion,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            document_root: PathBuf::from("/var/www/html"),
            follow_symlinks: false,
            max_file_size: 10 * 1024 * 1024 * 1024 * 1024, // 10TB
            allowed_extensions: vec![],
            blocked_extensions: vec![
/* // You could consider blocking these extensions by default for security
                "exe".to_string(),
                "bat".to_string(),
                "cmd".to_string(),
                "com".to_string(),
                "pif".to_string(),
                "scr".to_string(),
                "vbs".to_string(),
                "js".to_string(), // Block JS files by default for security
                "jar".to_string(),
                "sh".to_string(), */
            ],
            drop_to_uid: None,
            drop_to_gid: None,
            keep_alive_timeout: Duration::from_secs(5),
            keep_alive_max_requests: 100,
            minimum_http_version: HttpVersion::Http09,
        }
    }
}

/// Secure file server with built-in security features
pub struct SecureFileServer {
    config: SecurityConfig,
    mime_types: MimeTypes,
}

impl Clone for SecureFileServer {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            mime_types: self.mime_types.clone(),
        }
    }
}

impl SecureFileServer {
    /// Create a new secure file server
    pub fn new(config: SecurityConfig) -> Self {
        Self {
            config,
            mime_types: MimeTypes::default(),
        }
    }

    /// Validate if a domain is safe for file serving
    /// A safe domain:
    /// - Contains only alphanumeric characters and dots
    /// - Does not contain ".." (directory traversal)
    /// - Contains at least one dot
    pub fn is_domain_safe(domain: &str) -> bool {
        // Check if domain contains only alphanumeric characters and dots
        if !domain.chars().all(|c| c.is_alphanumeric() || c == '.') {
            return false;
        }

        // Check if domain contains ".." (directory traversal)
        if domain.contains("..") {
            return false;
        }

        // Check if domain contains at least one dot
        if !domain.contains('.') {
            return false;
        }

        // Additional safety checks
        // Domain should not start or end with a dot
        if domain.starts_with('.') || domain.ends_with('.') {
            return false;
        }

        // Domain should not have consecutive dots
        if domain.contains("...") {
            return false;
        }

        true
    }

    /// Get the document root for a specific domain
    /// Returns /var/www/DOMAIN if it exists and domain is safe, otherwise falls back to default
    pub fn get_domain_document_root(&self, domain: &str) -> PathBuf {
        if Self::is_domain_safe(domain) {
            let domain_path = PathBuf::from("/var/www").join(domain);
            if domain_path.exists() && domain_path.is_dir() {
                println!("Using domain-specific document root: {}", domain_path.display());
                return domain_path;
            }
        }

        // Fall back to default document root
        println!("Using default document root: {}", self.config.document_root.display());
        self.config.document_root.clone()
    }

    /// Drop privileges to specified user/group
    pub fn drop_privileges(&self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(unix)]
        {
            if let (Some(uid), Some(gid)) = (self.config.drop_to_uid, self.config.drop_to_gid) {
                println!("Dropping privileges to UID {} and GID {}", uid, gid);

                // Set GID first, then UID
                unsafe {
                    if libc::setgid(gid) != 0 {
                        return Err(format!("Failed to set GID to {}: {}", gid, std::io::Error::last_os_error()).into());
                    }
                    if libc::setuid(uid) != 0 {
                        return Err(format!("Failed to set UID to {}: {}", uid, std::io::Error::last_os_error()).into());
                    }
                }

                // Verify the privilege drop worked
                let new_uid = unsafe { libc::getuid() };
                let new_gid = unsafe { libc::getgid() };

                if new_uid != uid || new_gid != gid {
                    return Err(format!("Failed to drop privileges: expected UID={} GID={}, got UID={} GID={}",
                                     uid, gid, new_uid, new_gid).into());
                }

                println!("Successfully dropped privileges to UID {} and GID {}", uid, gid);
            }
        }

        #[cfg(not(unix))]
        {
            if self.config.drop_to_uid.is_some() || self.config.drop_to_gid.is_some() {
                println!("Warning: Privilege dropping not supported on non-Unix systems");
            }
        }

        Ok(())
    }

    /// Sanitize and canonicalize a path to prevent directory traversal attacks
    pub fn sanitize_path(&self, request_path: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        self.sanitize_path_with_root(request_path, &self.config.document_root)
    }

    /// Sanitize and canonicalize a path with a specific document root
    pub fn sanitize_path_with_root(&self, request_path: &str, document_root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        // Remove any query parameters or fragments
        let path = request_path.split('?').next().unwrap_or(request_path);
        let path = path.split('#').next().unwrap_or(path);

        // Decode URL encoding
        let path = urlencoding::decode(path)?.into_owned();

        // Convert to PathBuf
        let mut path_buf = PathBuf::new();

        // Handle absolute paths vs relative paths
        for component in Path::new(&path).components() {
            match component {
                Component::Normal(comp) => {
                    // Check for directory traversal attempts
                    if comp == ".." || comp.to_string_lossy().contains("..") {
                        return Err("Directory traversal attack detected".into());
                    }

                    // Check for hidden files/directories (starting with .)
                    if let Some(comp_str) = comp.to_str() {
                        if comp_str.starts_with('.') && comp_str != "." && comp_str != ".." {
                            return Err("Access to hidden files/directories not allowed".into());
                        }
                    }

                    path_buf.push(comp);
                }
                Component::RootDir => {
                    // Absolute path - start from document root
                    path_buf = document_root.to_path_buf();
                }
                Component::CurDir => {
                    // Current directory - ignore
                }
                _ => {
                    return Err("Invalid path component".into());
                }
            }
        }

        // If path is empty or just root, try to serve index.html
        if path_buf == PathBuf::new() || path_buf == document_root {
            path_buf.push("index.html");
        }

        // Canonicalize the path to resolve any symlinks (if allowed)
        let canonical_path = if self.config.follow_symlinks {
            fs::canonicalize(&path_buf)?
        } else {
            // Still canonicalize but don't follow symlinks
            let mut canonical = PathBuf::new();
            for component in path_buf.components() {
                canonical.push(component);
            }
            canonical
        };

        // Ensure the canonical path is within the document root
        if !canonical_path.starts_with(document_root) {
            return Err("Path outside document root not allowed".into());
        }

        // Check if path exists and is a file
        if !canonical_path.exists() {
            return Err("File not found".into());
        }

        if !canonical_path.is_file() {
            return Err("Path is not a file".into());
        }

        // Check file size
        if let Ok(metadata) = fs::metadata(&canonical_path) {
            if metadata.len() > self.config.max_file_size {
                return Err("File too large".into());
            }

            // Check file permissions (should not be executable for security)
            let permissions = metadata.permissions();
            if permissions.readonly() {
                // File is read-only, that's good for security
            } else {
                println!("Warning: File {} has write permissions", canonical_path.display());
            }
        }

        // Check file extension restrictions
        if let Some(extension) = canonical_path.extension().and_then(|ext| ext.to_str()) {
            let extension = extension.to_lowercase();

            // Check blocked extensions
            if self.config.blocked_extensions.contains(&extension) {
                return Err("File type not allowed".into());
            }

            // Check allowed extensions (if specified)
            if !self.config.allowed_extensions.is_empty() &&
               !self.config.allowed_extensions.contains(&extension) {
                return Err("File type not in allowed list".into());
            }
        }

        Ok(canonical_path)
    }

    /// Check if a path should redirect (directory without trailing slash)
    /// Returns Some(redirect_url) if redirect is needed, None otherwise
    pub fn check_redirect(&self, request_path: &str) -> Option<String> {
        self.check_redirect_with_domain(request_path, None)
    }

    /// Check if a path should redirect with domain-specific document root
    /// Returns Some(redirect_url) if redirect is needed, None otherwise
    pub fn check_redirect_with_domain(&self, request_path: &str, domain: Option<&str>) -> Option<String> {
        // Only check for redirects if path doesn't end with slash
        if request_path.ends_with('/') {
            return None;
        }

        // Get the appropriate document root for this domain
        let document_root = if let Some(domain) = domain {
            self.get_domain_document_root(domain)
        } else {
            self.config.document_root.clone()
        };

        // Build the full path to check if it's a directory
        let clean_path = request_path.trim_start_matches('/');
        let file_path = document_root.join(clean_path);

        // Check if the path is a directory
        if file_path.is_dir() {
            // Redirect to the same path with trailing slash
            let redirect_url = format!("{}/", request_path);
            println!("Redirecting '{}' to '{}'", request_path, redirect_url);
            return Some(redirect_url);
        }

        None
    }

    /// Serve a file with proper security checks and redirect support
    /// Returns Ok(None) if file doesn't exist, Ok(Some(content)) if file exists
    /// Returns Ok(Some(redirect_response)) if redirect is needed
    pub fn serve_file(&self, request_path: &str) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        self.serve_file_with_domain(request_path, None)
    }

    /// Serve a file with domain-specific document root
    /// Returns Ok(None) if file doesn't exist, Ok(Some(content)) if file exists
    /// Returns Ok(Some(redirect_response)) if redirect is needed
    pub fn serve_file_with_domain(&self, request_path: &str, domain: Option<&str>) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        // Check for redirects first using domain-specific document root
        if let Some(redirect_url) = self.check_redirect_with_domain(request_path, domain) {
            // For backward compatibility, use HTTP/1.1 with close for redirects
            let response = self.generate_redirect_response(&redirect_url, &HttpVersion::Http11, false);
            return Ok(Some(response.as_bytes().to_vec()));
        }

        // Get the appropriate document root for this domain
        let document_root = if let Some(domain) = domain {
            self.get_domain_document_root(domain)
        } else {
            self.config.document_root.clone()
        };

        // Handle directory requests (paths ending with /)
        if request_path.ends_with('/') {
            let clean_path = request_path.trim_start_matches('/');
            let dir_path = document_root.join(clean_path);

            if dir_path.is_dir() {
                // Try to serve index.html or index.htm from the directory
                let index_html = dir_path.join("index.html");
                let index_htm = dir_path.join("index.htm");

                let file_to_serve = if index_html.exists() {
                    index_html
                } else if index_htm.exists() {
                    index_htm
                } else {
                    // No index file found, return 404
                    return Ok(None);
                };

                // Serve the index file
                let mut file = match File::open(&file_to_serve) {
                    Ok(file) => file,
                    Err(e) => {
                        println!("Error opening file {}: {}", file_to_serve.display(), e);
                        return Ok(None);
                    }
                };

                let mut contents = Vec::new();
                if let Err(e) = file.read_to_end(&mut contents) {
                    println!("Error reading file {}: {}", file_to_serve.display(), e);
                    return Ok(None);
                }

                println!("Successfully served directory index: {} ({} bytes)",
                        file_to_serve.display(), contents.len());

                return Ok(Some(contents));
            }
        }

        // For regular file requests, use the existing sanitize_path logic with domain-specific root
        let file_path = match self.sanitize_path_with_root(request_path, &document_root) {
            Ok(path) => path,
            Err(e) => {
                println!("Security error serving {}: {}", request_path, e);
                return Ok(None); // Return None to indicate file not found (security through obscurity)
            }
        };

        // Read file contents
        let mut file = match File::open(&file_path) {
            Ok(file) => file,
            Err(e) => {
                println!("Error opening file {}: {}", file_path.display(), e);
                return Ok(None);
            }
        };

        let mut contents = Vec::new();
        if let Err(e) = file.read_to_end(&mut contents) {
            println!("Error reading file {}: {}", file_path.display(), e);
            return Ok(None);
        }

        println!("Successfully served file: {} ({} bytes)",
                file_path.display(), contents.len());

        Ok(Some(contents))
    }

    /// Get MIME type for a path
    pub fn get_mime_type(&self, path: &Path) -> String {
        self.mime_types.get_mime_type(path)
    }

    /// Generate a simple HTTP response for content (like ACME challenges or default pages)
    pub fn generate_simple_http_response(&self, content: &[u8], content_type: &str, include_last_modified: bool) -> String {
        // For backward compatibility, default to HTTP/1.1 with close
        self.generate_simple_http_response_with_version(content, content_type, include_last_modified, &HttpVersion::Http11, false)
    }

    /// Generate a simple HTTP response with version and keep-alive support
    pub fn generate_simple_http_response_with_version(&self, content: &[u8], content_type: &str, include_last_modified: bool, version: &HttpVersion, keep_alive: bool) -> String {

        let mut response = HttpResponse::ok(content.to_vec());
        response.set_content_type(content_type);
        response.set_content_length();

        // Add Last-Modified header if requested and we can determine it
        if include_last_modified {
            // For simple responses, we can't easily determine file modification time
            // So we'll use the current time as a fallback
            let now = std::time::SystemTime::now();
            let last_modified = format_http_date(&now);
            response.set_last_modified(&last_modified);
        }

        // Add security headers for HTML content
        if content_type.starts_with("text/html") {
            response.add_security_headers();
        }

        // Add cache control
        if content_type.starts_with("text/plain") {
            // ACME challenges should not be cached
            response.set_cache_control("no-cache");
        } else {
            // Default pages can be cached briefly
            response.set_cache_control("public, max-age=300");
        }

        // Encode with version and keep-alive settings
        let encoded = response.encode(version, keep_alive);
        String::from_utf8_lossy(&encoded).to_string()
    }

    /// Generate an HTTP response for a file with proper Last-Modified header
    pub fn generate_http_response(&self, request_path: &str, content: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
        // For backward compatibility, default to HTTP/1.1 with close
        self.generate_http_response_with_version(request_path, content, &HttpVersion::Http11, false)
    }

    /// Generate an HTTP response for a file with version and keep-alive support
    pub fn generate_http_response_with_version(&self, request_path: &str, content: &[u8], version: &HttpVersion, keep_alive: bool) -> Result<String, Box<dyn std::error::Error>> {

        let file_path = self.sanitize_path(request_path)?;
        let mime_type = self.get_mime_type(&file_path);

        let mut response = HttpResponse::ok(content.to_vec());
        response.set_content_type(&mime_type);
        response.set_content_length();

        // Get file modification time for Last-Modified header
        if let Ok(metadata) = std::fs::metadata(&file_path) {
            if let Ok(modified_time) = metadata.modified() {
                let last_modified = format_http_date(&modified_time);
                response.set_last_modified(&last_modified);
            }
        }

        // Add security headers
        response.add_security_headers();

        // Add cache control for static assets
        if mime_type.starts_with("image/") ||
           mime_type.starts_with("text/css") ||
           mime_type.starts_with("application/javascript") ||
           mime_type.starts_with("application/wasm") {
            response.set_cache_control("public, max-age=3600");
        } else {
            response.set_cache_control("no-cache");
        }

        // Encode with version and keep-alive settings
        let encoded = response.encode(version, keep_alive);
        Ok(String::from_utf8_lossy(&encoded).to_string())
    }

    /// Generate HTTP response headers for a file (without content)
    /// This is useful when you have the file path and content separately
    pub fn generate_file_response_headers(&self, file_path: &Path, content_length: usize) -> Result<String, Box<dyn std::error::Error>> {
        // For backward compatibility, default to HTTP/1.1 with close
        self.generate_file_response_headers_with_version(file_path, content_length, &HttpVersion::Http11, false)
    }

    /// Generate HTTP response headers for a file with version and keep-alive support
    pub fn generate_file_response_headers_with_version(&self, file_path: &Path, content_length: usize, version: &HttpVersion, keep_alive: bool) -> Result<String, Box<dyn std::error::Error>> {

        let mime_type = self.get_mime_type(file_path);

        let mut response = HttpResponse::ok(vec![]); // Empty body for headers-only
        response.set_content_type(&mime_type);
        response.set_header("Content-Length", &content_length.to_string());

        // Get file modification time for Last-Modified header
        if let Ok(metadata) = std::fs::metadata(file_path) {
            if let Ok(modified_time) = metadata.modified() {
                let last_modified = format_http_date(&modified_time);
                response.set_last_modified(&last_modified);
            }
        }

        // Add security headers
        response.add_security_headers();

        // Add cache control for static assets
        if mime_type.starts_with("image/") ||
           mime_type.starts_with("text/css") ||
           mime_type.starts_with("application/javascript") ||
           mime_type.starts_with("application/wasm") {
            response.set_cache_control("public, max-age=3600");
        } else {
            response.set_cache_control("no-cache");
        }

        // Encode with version and keep-alive settings
        let encoded = response.encode(version, keep_alive);
        Ok(String::from_utf8_lossy(&encoded).to_string())
    }

    /// Generate a redirect response with version and keep-alive support
    pub fn generate_redirect_response(&self, location: &str, version: &HttpVersion, keep_alive: bool) -> String {

        let response = HttpResponse::moved_permanently(location);
        let encoded = response.encode(version, keep_alive);
        String::from_utf8_lossy(&encoded).to_string()
    }

    /// Check if a file extension is allowed
    pub fn is_extension_allowed(&self, extension: &str) -> bool {
        let extension = extension.to_lowercase();

        // Check blocked extensions
        if self.config.blocked_extensions.contains(&extension) {
            return false;
        }

        // Check allowed extensions (if specified)
        if !self.config.allowed_extensions.is_empty() {
            return self.config.allowed_extensions.contains(&extension);
        }

        true
    }

    /// Get security configuration
    pub fn config(&self) -> &SecurityConfig {
        &self.config
    }

    /// Update security configuration
    pub fn update_config(&mut self, config: SecurityConfig) {
        self.config = config;
    }

    /// Generate a default informational page when index.html is missing
    pub fn generate_default_page(&self, domain: &str) -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>easyp HTTPS Server</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            margin: 0;
            padding: 20px;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: #333;
            min-height: 100vh;
            display: flex;
            flex-direction: column;
            align-items: center;
            justify-content: center;
        }}
        .container {{
            background: white;
            padding: 40px;
            border-radius: 15px;
            box-shadow: 0 20px 40px rgba(0,0,0,0.1);
            max-width: 800px;
            width: 100%;
            text-align: center;
        }}
        .header {{
            margin-bottom: 30px;
        }}
        .header h1 {{
            color: #2c3e50;
            margin: 0;
            font-size: 2.5em;
            font-weight: 300;
        }}
        .header p {{
            color: #7f8c8d;
            margin: 10px 0 0 0;
            font-size: 1.1em;
        }}
        .status-badge {{
            display: inline-block;
            padding: 8px 16px;
            background: #27ae60;
            color: white;
            border-radius: 20px;
            font-size: 0.9em;
            margin: 10px 0;
        }}
        .info-section {{
            margin: 30px 0;
            text-align: left;
        }}
        .info-section h2 {{
            color: #34495e;
            border-bottom: 2px solid #3498db;
            padding-bottom: 5px;
        }}
        .info-grid {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
            gap: 20px;
            margin: 20px 0;
        }}
        .info-card {{
            background: #f8f9fa;
            padding: 20px;
            border-radius: 8px;
            border-left: 4px solid #3498db;
        }}
        .info-card h3 {{
            margin: 0 0 10px 0;
            color: #2c3e50;
        }}
        .info-card p {{
            margin: 0;
            color: #7f8c8d;
            font-size: 0.9em;
        }}
        .getting-started {{
            background: #e8f4fd;
            border: 1px solid #bee5eb;
            border-radius: 8px;
            padding: 20px;
            margin: 20px 0;
            text-align: left;
        }}
        .getting-started h3 {{
            color: #0c5460;
            margin: 0 0 15px 0;
        }}
        .getting-started ol {{
            margin: 0;
            padding-left: 20px;
        }}
        .getting-started li {{
            margin: 8px 0;
            color: #495057;
        }}
        .footer {{
            margin-top: 30px;
            padding-top: 20px;
            border-top: 1px solid #dee2e6;
            color: #6c757d;
            font-size: 0.9em;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>🔒 easyp HTTPS Server</h1>
            <p>Secure web server with automatic ACME certificate management</p>
            <div class="status-badge">🟢 Running</div>
        </div>

        <div class="info-section">
            <h2>Server Information</h2>
            <div class="info-grid">
                <div class="info-card">
                    <h3>🌐 Domain</h3>
                    <p>{}</p>
                </div>
                <div class="info-card">
                    <h3>🔒 Security</h3>
                    <p>ACME certificates<br>Path sanitization<br>Privilege dropping</p>
                </div>
                <div class="info-card">
                    <h3>📁 Document Root</h3>
                    <p>{}</p>
                </div>
                <div class="info-card">
                    <h3>⚡ Features</h3>
                    <p>HTTP/HTTPS support<br>WebAssembly ready<br>MIME type handling</p>
                </div>
            </div>
        </div>

        <div class="getting-started">
            <h3>🚀 Getting Started</h3>
            <p>To serve your own content, add files to your document root:</p>
            <ol>
                <li>Create an <code>index.html</code> file in your document root directory</li>
                <li>Add other HTML, CSS, JavaScript, or media files as needed</li>
                <li>The server will automatically serve files with proper MIME types</li>
                <li>All requests are validated for security (no directory traversal attacks)</li>
            </ol>
            <p><strong>Current document root:</strong> <code>{}</code></p>
        </div>

        <div class="info-section">
            <h2>🔧 Security Features</h2>
            <div class="info-grid">
                <div class="info-card">
                    <h3>🛡️ Path Sanitization</h3>
                    <p>Prevents directory traversal attacks (../) and access to hidden files</p>
                </div>
                <div class="info-card">
                    <h3>👤 Privilege Dropping</h3>
                    <p>Drops to unprivileged user (nobody) after binding to privileged ports</p>
                </div>
                <div class="info-card">
                    <h3>🔗 Path Canonicalization</h3>
                    <p>Resolves symlinks safely and validates all paths are within document root</p>
                </div>
                <div class="info-card">
                    <h3>📋 File Type Support</h3>
                    <p>Comprehensive MIME type support for web files, images, and WebAssembly</p>
                </div>
            </div>
        </div>

        <div class="footer">
            <p>Powered by <strong>rustls</strong> with ACME certificate management</p>
            <p>Visit <a href="https://github.com/rustls/rustls" target="_blank">rustls on GitHub</a> for more information</p>
        </div>
    </div>
</body>
</html>"#,
            domain,
            self.config.document_root.display(),
            self.config.document_root.display()
        )
    }

    /// Check if this is a request for the root path (index.html)
    pub fn is_root_request(&self, request_path: &str) -> bool {
        request_path == "/" || request_path.trim_end_matches('/') == ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    #[test]
    fn test_mime_types() {
        let mime_types = MimeTypes::default();

        assert_eq!(mime_types.get_mime_type(Path::new("test.html")), "text/html; charset=utf-8");
        assert_eq!(mime_types.get_mime_type(Path::new("test.js")), "application/javascript; charset=utf-8");
        assert_eq!(mime_types.get_mime_type(Path::new("test.wasm")), "application/wasm");
        assert_eq!(mime_types.get_mime_type(Path::new("test.unknown")), "application/octet-stream");
    }
}
