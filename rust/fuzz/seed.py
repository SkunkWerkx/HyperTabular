"""Writes starting corpora for the fuzz targets into fuzz/corpus/, from the repository's own
conformance corpus: nothing is committed (HyperCast commits none either), and a seed set
this script can rebuild is one nobody has to keep in step by hand.

    python3 fuzz/seed.py        (from rust/)
"""

import base64
import hashlib
import json
import pathlib
import zlib

here = pathlib.Path(__file__).resolve().parent
repo = here.parent.parent
out = here / "corpus"


def write(target, data):
    folder = out / target
    folder.mkdir(parents=True, exist_ok=True)
    (folder / hashlib.sha1(data).hexdigest()).write_bytes(data)


# Delimited: each corpus case's input, behind the five selector bytes, in a few dialects.
separators = [",", "\t", ";", "|", " ", "a", "'"]
for case in json.loads((repo / "corpus/delimited.json").read_text()):
    dialect = case["dialect"]
    text = case["input"]
    data = base64.b64decode(text) if case.get("encoding") == "base64" else text.encode()
    sep = separators.index(dialect["separator"]) if dialect["separator"] in separators else 0
    sel = sep | (dialect["quoting"] << 3) | (dialect["skip_blank_lines"] << 4)
    sel |= 0x20 if dialect.get("has_header") else 0
    for sizes in (0x00, 0x37, 0xF5):
        write("delimited", bytes([sel, sizes, 0x02, 0x51, 0xA3]) + data)

# Workbooks: every package under 20 KB, whole, behind the two selector bytes.
for path in sorted((repo / "corpus/workbook").iterdir()):
    if path.suffix in (".xlsx", ".xlsm", ".ods") and path.stat().st_size < 20_000:
        write("workbook", bytes([0x01, 0x47]) + path.read_bytes())
        write("workbook", bytes([0x02, 0xA0]) + path.read_bytes())

# Sheets: small XML in the shapes the readers know.
xlsx_rows = (
    b'<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>x</t></is></c></row>'
    b'<row r="3"><c r="A3"><v>45000.5</v></c><c r="C3" t="b"><v>1</v></c>'
    b'<c r="D3" s="1"><v>1.5</v></c><c r="E3" t="e"><v>#N/A</v></c></row>'
)
strings = b"<si><t>id</t></si><si><r><t>a</t></r><r><t>b</t></r></si>"
for sel in (0x08, 0x0A, 0x1C, 0x60):
    write("sheet", bytes([sel]) + xlsx_rows + b"\x00" + strings)
ods_table = (
    b'<table:table table:name="T"><table:table-row><table:table-cell office:value-type="float" '
    b'office:value="1.5"/><table:table-cell office:value-type="string"><text:p>a<text:s '
    b'text:c="2"/>b</text:p></table:table-cell></table:table-row><table:table-row '
    b'table:number-rows-repeated="3"><table:table-cell table:number-columns-repeated="2" '
    b'office:value-type="date" office:date-value="2026-10-07T01:02:03"/></table:table-row>'
    b"</table:table>"
)
for sel in (0x01, 0x03, 0x19, 0x61):
    write("sheet", bytes([sel]) + ods_table)

# Inflate: deflate streams of the corpus's own text at several levels.
for text in [b"", b"a", b"abc" * 1000, (repo / "corpus/delimited.json").read_bytes()[:50_000]]:
    for level in (0, 1, 6, 9):
        compressor = zlib.compressobj(level, zlib.DEFLATED, -15)
        stream = compressor.compress(text) + compressor.flush()
        write("inflate", bytes([0x13]) + stream)
