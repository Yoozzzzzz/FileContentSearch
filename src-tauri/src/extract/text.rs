use super::*;

pub(super) fn decode(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if !bytes.len().is_multiple_of(2) {
            return Err(ExtractError::new(
                ErrorKind::Decode,
                "UTF-16 文件字节数不完整",
            ));
        }
        let little = bytes[0] == 0xff;
        let units = bytes[2..].as_chunks::<2>().0.iter().map(|pair| {
            if little {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        });
        char::decode_utf16(units)
            .collect::<std::result::Result<String, _>>()
            .map_err(|_| ExtractError::new(ErrorKind::Decode, "UTF-16 包含无效代理项"))
    } else {
        std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))
            .map(str::to_owned)
            .map_err(|_| {
                ExtractError::new(
                    ErrorKind::Decode,
                    "仅支持 UTF-8 或带 BOM 的 UTF-16；文件无法严格解码",
                )
            })
    }
}

pub(super) fn extract(bytes: &[u8], limits: Limits) -> Result<Document> {
    let text = decode(bytes)?;
    if text.contains('\0') {
        return Err(ExtractError::new(
            ErrorKind::Decode,
            "文本包含 NUL，可能是二进制文件或缺少编码标记",
        ));
    }
    if text.len() > limits.text_bytes {
        return Err(ExtractError::limit("提取文本超过字节上限"));
    }
    let mut document = Document::new(LocationKind::Line);
    document.mark(limits)?;
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            if document.locations.len() >= limits.locations {
                return Err(ExtractError::limit("行数超过上限"));
            }
            document.locations.push(Location {
                start: offset + 1,
                number: document.locations.len() as u32 + 1,
            });
        }
    }
    document.text = text;
    Ok(document)
}
