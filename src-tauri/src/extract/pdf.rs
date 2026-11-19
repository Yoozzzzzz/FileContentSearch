use super::*;
use std::io::{self, Write};

struct LimitedOutput<'a> {
    document: &'a mut Document,
    limits: Limits,
    exceeded: bool,
}

impl Write for LimitedOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
        if self.document.push(text, self.limits).is_err() {
            self.exceeded = true;
            return Err(io::Error::other("text limit"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn extract(bytes: &[u8], limits: Limits) -> Result<Document> {
    // pdf-extract contains assertions; a single malformed document must fail the file, not the batch.
    std::panic::catch_unwind(|| extract_inner(bytes, limits))
        .map_err(|_| ExtractError::corrupt("PDF 解析器遇到不支持或损坏的文档"))?
}

fn extract_inner(bytes: &[u8], limits: Limits) -> Result<Document> {
    // Disk input is parsed in the memory-limited worker; the loader allocates before returning.
    let mut pdf = pdf_extract::Document::load_mem_with_options(
        bytes,
        pdf_extract::LoadOptions {
            strict: true,
            ..Default::default()
        },
    )
    .map_err(|error| {
        if matches!(error, pdf_extract::Error::Decryption(_)) {
            ExtractError::new(ErrorKind::Encrypted, "PDF 已加密，暂不提供密码解密")
        } else {
            ExtractError::corrupt("PDF 文档结构损坏或不受支持")
        }
    })?;
    if pdf.is_encrypted() || pdf.trailer.has(b"Encrypt") {
        return Err(ExtractError::new(
            ErrorKind::Encrypted,
            "PDF 已加密，暂不提供密码解密",
        ));
    }
    let mut decoded_bytes = 0usize;
    for object in pdf.objects.values_mut() {
        if let pdf_extract::Object::Stream(stream) = object {
            if stream
                .dict
                .get(b"Subtype")
                .and_then(pdf_extract::Object::as_name)
                .ok()
                == Some(b"Image")
            {
                continue;
            }
            let decoded = stream
                .decompressed_content()
                .map_err(|_| ExtractError::corrupt("PDF 内容流无法解码"))?;
            decoded_bytes = decoded_bytes
                .checked_add(decoded.len())
                .ok_or_else(|| ExtractError::limit("PDF 解压量超过上限"))?;
            if decoded_bytes > limits.pdf_decoded_bytes {
                return Err(ExtractError::limit("PDF 非图像流累计解压量超过上限"));
            }
            stream.set_plain_content(decoded);
        }
    }
    let pages = pdf.get_pages();
    if pages.len() > limits.pdf_pages {
        return Err(ExtractError::limit("PDF 页数超过上限"));
    }
    let mut document = Document::new(LocationKind::Page);
    for (page, object_id) in pages {
        let dictionary = pdf
            .get_dictionary(object_id)
            .map_err(|_| ExtractError::corrupt("PDF 页面对象损坏"))?;
        if let Ok(contents) = dictionary.get(b"Contents") {
            validate_contents(&pdf, contents, 0)?;
        }
        if !document.locations.is_empty() {
            document.push("\n", limits)?;
        }
        document.mark(limits)?;
        let mut output = LimitedOutput {
            document: &mut document,
            limits,
            exceeded: false,
        };
        // The by-pages convenience functions stop at the first page error and return partial success.
        let result = pdf_extract::output_doc_page(
            &pdf,
            &mut pdf_extract::PlainTextOutput::new(&mut output as &mut dyn Write),
            page,
        );
        if output.exceeded {
            return Err(ExtractError::limit("PDF 提取文本超过上限，未保存截断内容"));
        }
        result.map_err(|_| ExtractError::corrupt("PDF 页面提取失败，未保存部分内容"))?;
    }
    Ok(document)
}

fn validate_contents(
    pdf: &pdf_extract::Document,
    object: &pdf_extract::Object,
    depth: usize,
) -> Result<()> {
    use pdf_extract::Object;
    if depth > 64 {
        return Err(ExtractError::limit("PDF 内容引用嵌套超过上限"));
    }
    match object {
        Object::Reference(id) => validate_contents(
            pdf,
            pdf.objects
                .get(id)
                .ok_or_else(|| ExtractError::corrupt("PDF 页面内容引用丢失"))?,
            depth + 1,
        ),
        Object::Stream(_) | Object::Null => Ok(()),
        Object::Array(items) => {
            for item in items {
                let id = item
                    .as_reference()
                    .map_err(|_| ExtractError::corrupt("PDF 页面内容数组包含无效对象"))?;
                pdf.get_object(id)
                    .and_then(Object::as_stream)
                    .map_err(|_| ExtractError::corrupt("PDF 页面内容数组引用的对象不是内容流"))?;
            }
            Ok(())
        }
        _ => Err(ExtractError::corrupt(
            "PDF 页面内容不是有效内容流，未保存部分文本",
        )),
    }
}
