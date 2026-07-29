use anyhow::{anyhow, Context, Result};
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::tunnel::TunnelStream;

const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Serve one HTTP/1.1 GET or HEAD request over a tunnel.
///
/// For a static service, localAddr is a directory rather than a TCP address.
/// Every request gets its own tunnel, so closing the response closes the
/// connection just like a small nginx static site.
pub async fn serve(mut tunnel: TunnelStream, root: &str) -> Result<()> {
    let (method, target) = match read_request(&mut tunnel).await {
        Ok(request) => request,
        Err(error) => {
            write_error(&mut tunnel, 400, "Bad Request", "bad request").await?;
            return Err(error);
        }
    };

    if method != "GET" && method != "HEAD" {
        write_error(
            &mut tunnel,
            405,
            "Method Not Allowed",
            "only GET and HEAD are supported",
        )
        .await?;
        return Ok(());
    }

    let root = match std::fs::canonicalize(root) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            write_error(
                &mut tunnel,
                500,
                "Internal Server Error",
                "static root is not a directory",
            )
            .await?;
            return Ok(());
        }
        Err(_) => {
            write_error(
                &mut tunnel,
                500,
                "Internal Server Error",
                "static root is unavailable",
            )
            .await?;
            return Ok(());
        }
    };

    let path = match resolve_path(&root, &target) {
        Ok(path) => path,
        Err(PathError::BadRequest) => {
            write_error(&mut tunnel, 400, "Bad Request", "invalid path").await?;
            return Ok(());
        }
        Err(PathError::NotFound) => {
            write_error(&mut tunnel, 404, "Not Found", "not found").await?;
            return Ok(());
        }
    };

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => {
            write_error(&mut tunnel, 404, "Not Found", "not found").await?;
            return Ok(());
        }
    };

    let content_type = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        metadata.len(),
        content_type,
    );
    tunnel.write_all(headers.as_bytes()).await?;

    if method == "GET" {
        let mut file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("open static file {}", path.display()))?;
        tokio::io::copy(&mut file, &mut tunnel).await?;
    }
    tunnel.flush().await?;
    Ok(())
}

async fn read_request(tunnel: &mut TunnelStream) -> Result<(String, String)> {
    let mut bytes = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    while bytes.len() < MAX_HEADER_BYTES {
        if tunnel.read(&mut byte).await? == 0 {
            return Err(anyhow!("connection closed before HTTP headers"));
        }
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes).context("HTTP headers are not UTF-8")?;
            let request_line = headers
                .split("\r\n")
                .next()
                .ok_or_else(|| anyhow!("missing request line"))?;
            let mut fields = request_line.split_whitespace();
            let method = fields
                .next()
                .ok_or_else(|| anyhow!("missing HTTP method"))?
                .to_ascii_uppercase();
            let target = fields
                .next()
                .ok_or_else(|| anyhow!("missing HTTP target"))?
                .to_string();
            if fields.next().is_none() {
                return Err(anyhow!("missing HTTP version"));
            }
            return Ok((method, target));
        }
    }
    Err(anyhow!("HTTP headers exceed {MAX_HEADER_BYTES} bytes"))
}

enum PathError {
    BadRequest,
    NotFound,
}

fn resolve_path(root: &Path, target: &str) -> std::result::Result<PathBuf, PathError> {
    let raw_path = target.split_once('?').map_or(target, |(path, _)| path);
    if !raw_path.starts_with('/') {
        return Err(PathError::BadRequest);
    }

    let decoded = percent_decode(raw_path).ok_or(PathError::BadRequest)?;
    if decoded.contains('\0') || decoded.contains('\\') {
        return Err(PathError::BadRequest);
    }

    let mut relative = PathBuf::new();
    for component in Path::new(decoded.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PathError::BadRequest);
            }
        }
    }

    let wants_index = decoded.ends_with('/');
    let mut candidate = root.join(relative);
    if wants_index || candidate.is_dir() {
        candidate.push("index.html");
    }

    let canonical = std::fs::canonicalize(&candidate).map_err(|_| PathError::NotFound)?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        return Err(PathError::NotFound);
    }
    Ok(canonical)
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

async fn write_error(
    tunnel: &mut TunnelStream,
    status: u16,
    reason: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tunnel.write_all(response.as_bytes()).await?;
    tunnel.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{percent_decode, resolve_path, PathError};
    use std::path::Path;

    #[test]
    fn decodes_url_paths() {
        assert_eq!(
            percent_decode("/hello%20world.txt"),
            Some("/hello world.txt".to_string())
        );
        assert_eq!(percent_decode("/%ZZ"), None);
    }

    #[test]
    fn rejects_path_traversal_before_file_lookup() {
        let root = Path::new(".");
        assert!(matches!(
            resolve_path(root, "/../secret"),
            Err(PathError::BadRequest)
        ));
        assert!(matches!(
            resolve_path(root, "/%2e%2e/secret"),
            Err(PathError::BadRequest)
        ));
        assert!(matches!(
            resolve_path(root, "/C:%5cWindows"),
            Err(PathError::BadRequest)
        ));
    }
}
