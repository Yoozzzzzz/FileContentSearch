use super::*;
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use std::{collections::HashSet, io::Cursor};

const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const STRICT_WORD: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
const OFFICE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const STRICT_OFFICE: &str =
    "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument";

fn xml_error(_: impl fmt::Display) -> ExtractError {
    ExtractError::corrupt("DOCX XML 损坏或编码不受支持")
}

fn read_entry(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    remaining: &mut usize,
) -> Result<String> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| ExtractError::corrupt("DOCX 缺少所需条目或条目无法读取"))?;
    if entry.encrypted() {
        return Err(ExtractError::new(ErrorKind::Encrypted, "DOCX 条目已加密"));
    }
    if entry.size() > *remaining as u64 {
        return Err(ExtractError::limit("DOCX XML 解压量超过上限"));
    }
    let mut bytes = Vec::new();
    entry
        .by_ref()
        .take(*remaining as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ExtractError::corrupt("DOCX 条目解压或校验失败"))?;
    if bytes.len() > *remaining {
        return Err(ExtractError::limit("DOCX 实际解压量超过上限"));
    }
    *remaining -= bytes.len();
    text::decode(&bytes)
}

fn in_namespace(resolved: &ResolveResult<'_>, expected: &str) -> bool {
    matches!(resolved, ResolveResult::Bound(namespace) if namespace.as_ref() == expected)
}

fn main_part(xml: &str, limits: Limits) -> Result<String> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0;
    let mut target = None;
    let mut root = false;
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(xml_error)?;
        match event {
            Event::Start(element) => {
                depth += 1;
                if depth > limits.xml_depth {
                    return Err(ExtractError::limit("DOCX XML 嵌套超过上限"));
                }
                if depth == 1 {
                    if root
                        || !in_namespace(&namespace, REL)
                        || element.local_name().as_ref() != "Relationships"
                    {
                        return Err(ExtractError::corrupt("DOCX 关系根节点无效"));
                    }
                    root = true;
                }
                if depth == 2
                    && in_namespace(&namespace, REL)
                    && element.local_name().as_ref() == "Relationship"
                {
                    let mut kind = String::new();
                    let mut path = String::new();
                    let mut external = false;
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(xml_error)?;
                        let value = attribute
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(xml_error)?;
                        match attribute.key.as_ref() {
                            "Type" => kind = value.into_owned(),
                            "Target" => path = value.into_owned(),
                            "TargetMode" => external = value != "Internal",
                            _ => {}
                        }
                    }
                    if kind == OFFICE || kind == STRICT_OFFICE {
                        if external {
                            return Err(ExtractError::corrupt("DOCX 主文档不能是外部关系"));
                        }
                        let path = path.trim_start_matches('/');
                        if path.is_empty()
                            || path.contains(['\\', ':', '?', '#', '%'])
                            || path
                                .split('/')
                                .any(|part| part.is_empty() || part == "." || part == "..")
                        {
                            return Err(ExtractError::corrupt("DOCX 主文档关系路径无效或不受支持"));
                        }
                        if target.replace(path.to_owned()).is_some() {
                            return Err(ExtractError::corrupt("DOCX 存在多个主文档关系"));
                        }
                    }
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| ExtractError::corrupt("DOCX XML 结构无效"))?;
            }
            Event::DocType(_) => return Err(ExtractError::corrupt("DOCX 禁止 DTD 和外部实体")),
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(ExtractError::corrupt("DOCX 关系 XML 未闭合"));
    }
    target.ok_or_else(|| ExtractError::corrupt("DOCX 缺少主文档关系"))
}

fn parse_document(xml: &str, limits: Limits) -> Result<Document> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut document = Document::new(LocationKind::Paragraph);
    let mut depth: usize = 0;
    let mut root_seen = false;
    let mut body_seen = false;
    let mut body_depth = None;
    let mut text_depth = None;
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(xml_error)?;
        let word = in_namespace(&namespace, WORD) || in_namespace(&namespace, STRICT_WORD);
        match event {
            Event::Start(element) => {
                depth += 1;
                if depth > limits.xml_depth {
                    return Err(ExtractError::limit("DOCX XML 嵌套超过上限"));
                }
                let local = element.local_name();
                if depth == 1 {
                    if root_seen || !word || local.as_ref() != "document" {
                        return Err(ExtractError::corrupt("DOCX 主文档根节点无效"));
                    }
                    root_seen = true;
                }
                if word && depth == 2 && local.as_ref() == "body" {
                    if body_seen {
                        return Err(ExtractError::corrupt("DOCX 包含多个正文节点"));
                    }
                    body_seen = true;
                    body_depth = Some(depth);
                }
                if word && body_depth.is_some() {
                    match local.as_ref() {
                        "p" => document.mark(limits)?,
                        "t" => text_depth = Some(depth),
                        "tab" => document.push("\t", limits)?,
                        "br" | "cr" => document.push("\n", limits)?,
                        _ => {}
                    }
                }
            }
            Event::End(element) => {
                if word && body_depth.is_some() {
                    match element.local_name().as_ref() {
                        "p" | "tr" => document.push("\n", limits)?,
                        "tc" => document.push("\t", limits)?,
                        _ => {}
                    }
                }
                if text_depth == Some(depth) {
                    text_depth = None;
                }
                if body_depth == Some(depth) {
                    body_depth = None;
                }
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| ExtractError::corrupt("DOCX XML 结构无效"))?;
            }
            Event::Text(value) if text_depth.is_some() => {
                document.push(&value.xml10_content(), limits)?
            }
            Event::CData(value) if text_depth.is_some() => {
                document.push(&value.xml10_content(), limits)?
            }
            Event::GeneralRef(value) => {
                let decoded = quick_xml::escape::unescape(&format!("&{};", value.as_ref()))
                    .map_err(xml_error)?
                    .into_owned();
                if text_depth.is_some() {
                    document.push(&decoded, limits)?;
                }
            }
            Event::DocType(_) => return Err(ExtractError::corrupt("DOCX 禁止 DTD 和外部实体")),
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 || !root_seen || !body_seen {
        return Err(ExtractError::corrupt("DOCX 正文 XML 不完整"));
    }
    Ok(document)
}

pub(super) fn extract(bytes: &[u8], limits: Limits) -> Result<Document> {
    if bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0]) {
        return Err(ExtractError::new(
            ErrorKind::Encrypted,
            "文件为加密 Office 容器或旧格式，无法作为 DOCX 提取",
        ));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| ExtractError::corrupt("DOCX 不是有效的 ZIP 文档包"))?;
    if archive.len() > limits.zip_entries {
        return Err(ExtractError::limit("DOCX 压缩条目数超过上限"));
    }
    let mut names = HashSet::new();
    for name in archive.file_names() {
        if !names.insert(name.map_err(xml_error)?) {
            return Err(ExtractError::corrupt("DOCX 包含重复条目"));
        }
    }
    let mut remaining = limits.xml_bytes;
    let relationships = read_entry(&mut archive, "_rels/.rels", &mut remaining)?;
    let part = main_part(&relationships, limits)?;
    let xml = read_entry(&mut archive, &part, &mut remaining)?;
    parse_document(&xml, limits)
}
