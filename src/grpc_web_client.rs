//! gRPC-Web client using reqwest (HTTP/1.1).
//!
//! CF Containers does not support HTTP/2 (native gRPC), so we use gRPC-Web
//! protocol which works over HTTP/1.1.
//!
//! Frame format:
//! - Data frame:    [0x00][4-byte BE length][proto bytes]
//! - Trailer frame: [0x80][4-byte BE length][header text]

use prost::Message;
use tracing::{debug, warn};

/// Encode a proto message into gRPC-Web data frame.
fn encode_grpc_web_frame(msg: &impl Message) -> Vec<u8> {
    let proto_bytes = msg.encode_to_vec();
    let len = proto_bytes.len() as u32;
    let mut buf = Vec::with_capacity(5 + proto_bytes.len());
    buf.push(0x00); // data frame flag
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&proto_bytes);
    buf
}

/// Parse gRPC-Web response body into (data_bytes, grpc_status, grpc_message).
fn parse_grpc_web_response(body: &[u8]) -> Result<(Vec<u8>, i32, String), String> {
    let mut offset = 0;
    let mut data_bytes: Vec<u8> = Vec::new();
    let mut grpc_status: i32 = 0;
    let mut grpc_message = String::new();

    while offset + 5 <= body.len() {
        let flag = body[offset];
        let len = u32::from_be_bytes([
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
            body[offset + 4],
        ]) as usize;
        offset += 5;

        if offset + len > body.len() {
            return Err(format!(
                "Frame truncated: need {} bytes at offset {}, have {}",
                len,
                offset,
                body.len() - offset
            ));
        }

        let payload = &body[offset..offset + len];
        offset += len;

        if flag & 0x80 == 0 {
            // Data frame
            data_bytes = payload.to_vec();
        } else {
            // Trailer frame — parse "key: value\r\n" lines
            let trailer_text = String::from_utf8_lossy(payload);
            for line in trailer_text.lines() {
                if let Some(value) = line.strip_prefix("grpc-status:") {
                    grpc_status = value.trim().parse().unwrap_or(2);
                } else if let Some(value) = line.strip_prefix("grpc-message:") {
                    grpc_message = value.trim().to_string();
                }
            }
        }
    }

    Ok((data_bytes, grpc_status, grpc_message))
}

/// Make a unary gRPC-Web call via HTTP/1.1 POST.
///
/// # Arguments
/// * `client` - reqwest HTTP client
/// * `base_url` - e.g. "https://api-container.mtamaramu.com"
/// * `service_path` - e.g. "/logi.dtakologs.DtakologsService/BulkCreate"
/// * `request` - proto request message
/// * `org_id` - organization ID for x-organization-id header
pub async fn grpc_web_call<Req: Message, Resp: Message + Default>(
    client: &reqwest::Client,
    base_url: &str,
    service_path: &str,
    request: &Req,
    org_id: &str,
) -> Result<Resp, String> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), service_path);
    let body = encode_grpc_web_frame(request);

    debug!("gRPC-Web POST {} ({} bytes)", url, body.len());

    let mut req = client
        .post(&url)
        .header("content-type", "application/grpc-web+proto")
        .header("x-custom-header", "grpc-web-client")
        .body(body);

    if !org_id.is_empty() {
        req = req.header("x-organization-id", org_id);
    }

    let response = req
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, body_text));
    }

    let response_body = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let (data_bytes, grpc_status, grpc_message) = parse_grpc_web_response(&response_body)?;

    if grpc_status != 0 {
        // grpc-status 0 = OK
        return Err(format!(
            "gRPC error (status={}): {}",
            grpc_status, grpc_message
        ));
    }

    if data_bytes.is_empty() {
        warn!("gRPC-Web response has empty data frame, attempting to decode default");
    }

    Resp::decode(data_bytes.as_slice())
        .map_err(|e| format!("Failed to decode response proto: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_frame() {
        // Create a simple proto-like message using raw bytes
        let data = b"hello";
        let mut frame = Vec::new();
        frame.push(0x00);
        frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
        frame.extend_from_slice(data);

        // Add trailer
        let trailer = b"grpc-status: 0\r\ngrpc-message: OK\r\n";
        frame.push(0x80);
        frame.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
        frame.extend_from_slice(trailer);

        let (data_bytes, status, message) = parse_grpc_web_response(&frame).unwrap();
        assert_eq!(data_bytes, b"hello");
        assert_eq!(status, 0);
        assert_eq!(message, "OK");
    }

    #[test]
    fn test_parse_error_response() {
        let trailer = b"grpc-status: 13\r\ngrpc-message: internal error\r\n";
        let mut frame = Vec::new();
        frame.push(0x80);
        frame.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
        frame.extend_from_slice(trailer);

        let (data_bytes, status, message) = parse_grpc_web_response(&frame).unwrap();
        assert!(data_bytes.is_empty());
        assert_eq!(status, 13);
        assert_eq!(message, "internal error");
    }
}
