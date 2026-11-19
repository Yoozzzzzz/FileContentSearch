"""Rebuild deterministic synthetic fixtures; no third-party Python packages or private files."""
from pathlib import Path
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED
from io import BytesIO

ROOT = Path(__file__).resolve().parents[1] / "src-tauri" / "tests" / "fixtures"
ROOT.mkdir(parents=True, exist_ok=True)

TEXT = '第一行：合同签署 ABC "AND" %_ 😀\n第二行：正文命中，文件名不包含关键词。'
(ROOT / "chinese.txt").write_bytes(TEXT.encode("utf-8"))


def docx(xml, target="custom/main.xml", relation=None, extras=()):
    output = BytesIO()
    with ZipFile(output, "w", compression=ZIP_DEFLATED) as archive:
        entries = [
            ("[Content_Types].xml", '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/custom/main.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>'),
            ("_rels/.rels", relation or f'<r:Relationships xmlns:r="http://schemas.openxmlformats.org/package/2006/relationships"><r:Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="{target}"/></r:Relationships>'),
            (target, xml),
            *extras,
        ]
        for name, content in entries:
            item = ZipInfo(name, (2026, 1, 1, 0, 0, 0))
            item.compress_type = ZIP_DEFLATED
            archive.writestr(item, content.encode("utf-8"))
    return output.getvalue()


XML = '''<?xml version="1.0" encoding="UTF-8"?>
<d:document xmlns:d="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<d:body><d:p><d:r><d:t>合同</d:t></d:r><d:r><d:t xml:space="preserve">签署 ABC &amp; &lt;测试&gt; &#x1F600;</d:t></d:r></d:p>
<d:tbl><d:tr><d:tc><d:p><d:r><d:t>表格甲</d:t></d:r></d:p></d:tc><d:tc><d:p><d:r><d:t>表格乙</d:t></d:r></d:p></d:tc></d:tr></d:tbl>
<d:p><d:r><d:t>末段</d:t><d:tab/><d:t>制表</d:t><d:br/><d:t>换行</d:t></d:r></d:p></d:body></d:document>'''
(ROOT / "chinese.docx").write_bytes(docx(XML))
(ROOT / "external.docx").write_bytes(docx(XML, relation='<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="https://invalid.example/document.xml" TargetMode="External"/></Relationships>'))
(ROOT / "entity.docx").write_bytes(docx('<!DOCTYPE document [<!ENTITY xxe SYSTEM "file:///never-read">]>' + XML.split('?>', 1)[1].replace('合同', '&xxe;')))
(ROOT / "truncated.docx").write_bytes(docx(XML[:-15]))
(ROOT / "wrong-namespace.docx").write_bytes(docx(XML.replace('http://schemas.openxmlformats.org/wordprocessingml/2006/main', 'urn:unrelated')))
(ROOT / "deep.docx").write_bytes(docx(XML.replace('合同', '<d:r>' * 140 + '深' + '</d:r>' * 140)))
(ROOT / "strict.docx").write_bytes(docx(XML.replace('http://schemas.openxmlformats.org/wordprocessingml/2006/main', 'http://purl.oclc.org/ooxml/wordprocessingml/main')))


def pdf(pages, broken_second=False, encrypted=False):
    # Type0 + explicit ToUnicode mapping; the CIDs deliberately differ from Unicode.
    chars = list(dict.fromkeys(''.join(''.join(column) for column in pages)))
    mapping = {char: index + 1 for index, char in enumerate(chars)}
    cmap = '/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Test def /CMapType 2 def\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n'
    cmap += f'{len(chars)} beginbfchar\n'
    cmap += ''.join(f'<{mapping[char]:04X}> <{char.encode("utf-16-be").hex().upper()}>\n' for char in chars)
    cmap += 'endbfchar\nendcmap CMapName currentdict /CMap defineresource pop end end'

    def stream(data):
        data = data.encode("ascii")
        return f'<< /Length {len(data)} >>\nstream\n'.encode() + data + b'\nendstream'

    objects = [
        b'<< /Type /Catalog /Pages 2 0 R >>',
        f'<< /Type /Pages /Kids [{" ".join(f"{7 + i * 2} 0 R" for i in range(len(pages)))}] /Count {len(pages)} >>'.encode(),
        b'<< /Type /Font /Subtype /Type0 /BaseFont /TestFont /Encoding /Identity-H /DescendantFonts [4 0 R] /ToUnicode 6 0 R >>',
        b'<< /Type /Font /Subtype /CIDFontType2 /BaseFont /TestFont /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R /DW 1000 >>',
        b'<< /Type /FontDescriptor /FontName /TestFont /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 800 /StemV 80 >>',
        stream(cmap),
    ]
    for index, columns in enumerate(pages):
        content_id = 8 + index * 2
        objects.append(f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>'.encode())
        content = []
        for col, value in enumerate(columns):
            encoded = ''.join(f'{mapping[char]:04X}' for char in value)
            content.append(f'BT /F1 12 Tf 1 0 0 1 {40 + col * 280} 740 Tm <{encoded}> Tj ET')
        objects.append(b'42' if broken_second and index == 1 else stream('\n'.join(content)))
    encryption = ''
    if encrypted:
        objects.append(b'<< /Filter /Standard /V 1 /R 2 /Length 40 /O <' + b'00' * 32 + b'> /U <' + b'00' * 32 + b'> /P -4 >>')
        encryption = f'/Encrypt {len(objects)} 0 R /ID [<0001020304050607><0001020304050607>]'
    result = bytearray(b'%PDF-1.4\n%\xe2\xe3\xcf\xd3\n')
    offsets = [0]
    for index, obj in enumerate(objects, 1):
        offsets.append(len(result))
        result.extend(f'{index} 0 obj\n'.encode() + obj + b'\nendobj\n')
    start = len(result)
    result.extend(f'xref\n0 {len(objects) + 1}\n0000000000 65535 f \n'.encode())
    for offset in offsets[1:]:
        result.extend(f'{offset:010d} 00000 n \n'.encode())
    result.extend(f'trailer\n<< /Size {len(objects) + 1} /Root 1 0 R {encryption} >>\nstartxref\n{start}\n%%EOF\n'.encode())
    assert result[offsets[1]:].startswith(b'1 0 obj')
    return bytes(result)


(ROOT / 'chinese.pdf').write_bytes(pdf([['合同签署 ABC 😀'], ['第二页：合同验收']]))
(ROOT / 'columns.pdf').write_bytes(pdf([['左栏合同', '右栏验收']]))
(ROOT / 'empty.pdf').write_bytes(pdf([['']]))
(ROOT / 'broken-page.pdf').write_bytes(pdf([['第一页正常'], ['第二页损坏']], broken_second=True))
(ROOT / 'encrypted.pdf').write_bytes(pdf([['加密测试']], encrypted=True))
print(f'Generated {len(list(ROOT.iterdir()))} synthetic fixtures.')
