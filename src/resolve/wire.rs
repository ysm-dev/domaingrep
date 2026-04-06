pub const DNS_HEADER_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHeader {
    pub id: u16,
    pub rcode: u8,
    pub answer_count: u16,
}

pub fn encode_name(name: &str, buf: &mut [u8]) -> Option<usize> {
    let trimmed = name.trim_end_matches('.');
    if trimmed.is_empty() {
        return None;
    }

    let mut offset = 0usize;
    for label in trimmed.split('.') {
        if label.is_empty() || label.len() > 63 || offset + 1 + label.len() >= buf.len() {
            return None;
        }

        buf[offset] = label.len() as u8;
        offset += 1;
        buf[offset..offset + label.len()].copy_from_slice(label.as_bytes());
        offset += label.len();
    }

    if offset >= 255 || offset >= buf.len() {
        return None;
    }

    buf[offset] = 0;
    Some(offset + 1)
}

pub fn build_query(buf: &mut [u8], id: u16, name: &str, qtype: u16) -> Option<usize> {
    if buf.len() < DNS_HEADER_LEN + 5 {
        return None;
    }

    let name_len = encode_name(name, &mut buf[DNS_HEADER_LEN..])?;
    let after_name = DNS_HEADER_LEN + name_len;
    if after_name + 4 > buf.len() {
        return None;
    }

    buf[..2].copy_from_slice(&id.to_be_bytes());
    buf[2..4].copy_from_slice(&0x0100u16.to_be_bytes());
    buf[4..6].copy_from_slice(&1u16.to_be_bytes());
    buf[6..8].copy_from_slice(&0u16.to_be_bytes());
    buf[8..10].copy_from_slice(&0u16.to_be_bytes());
    buf[10..12].copy_from_slice(&0u16.to_be_bytes());
    buf[after_name..after_name + 2].copy_from_slice(&qtype.to_be_bytes());
    buf[after_name + 2..after_name + 4].copy_from_slice(&1u16.to_be_bytes());

    Some(after_name + 4)
}

pub fn parse_response_header(buf: &[u8]) -> Option<ResponseHeader> {
    if buf.len() < DNS_HEADER_LEN || (buf[2] & 0x80) == 0 {
        return None;
    }

    Some(ResponseHeader {
        id: u16::from_be_bytes([buf[0], buf[1]]),
        rcode: buf[3] & 0x0f,
        answer_count: u16::from_be_bytes([buf[6], buf[7]]),
    })
}

#[cfg(test)]
mod tests {
    use super::{build_query, encode_name, parse_response_header};

    #[test]
    fn encodes_names_in_dns_wire_format() {
        let mut buf = [0u8; 32];
        let len = encode_name("abc.com", &mut buf).unwrap();
        assert_eq!(&buf[..len], &[3, b'a', b'b', b'c', 3, b'c', b'o', b'm', 0]);
    }

    #[test]
    fn builds_queries() {
        let mut buf = [0u8; 64];
        let len = build_query(&mut buf, 0x1234, "abc.com", 2).unwrap();
        assert_eq!(u16::from_be_bytes([buf[0], buf[1]]), 0x1234);
        assert_eq!(u16::from_be_bytes([buf[4], buf[5]]), 1);
        assert_eq!(u16::from_be_bytes([buf[len - 4], buf[len - 3]]), 2);
    }

    #[test]
    fn parses_response_headers() {
        let header = parse_response_header(&[
            0x12, 0x34, 0x81, 0x83, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00,
        ])
        .unwrap();
        assert_eq!(header.id, 0x1234);
        assert_eq!(header.rcode, 3);
        assert_eq!(header.answer_count, 2);
    }
}
