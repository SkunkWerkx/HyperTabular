# Runs inside the ruby.wasm interpreter `rbwasm build` made from ./Gemfile, under Node
# (node-test.mjs) and in headless Chrome (index.html). Raises on the first failed check, so
# both hosts see an exception rather than a page to read; returns the report otherwise.
require "/bundle/setup"
require "hypertabular"

checks = []
check = lambda do |label, got, want|
  raise "FAIL #{label}: got #{got.inspect}, want #{want.inspect}" unless got == want

  checks << "ok #{label} (#{got.inspect})"
end
column = HyperTabular::Column

check.("platform", RUBY_PLATFORM.include?("wasm32-wasi"), true)
check.("Magnus backend", HyperTabular::BACKEND, :native)
check.("HyperCast's Magnus backend beside it", HyperCast::BACKEND, :native)
check.("core version matches the gem", HyperTabular.native_version, HyperTabular::VERSION)

csv = "id,amount,name\n1,\"1,234.50\",alice\n2,7,\"y\"\"z\"\n1€,0,é\n"
reader = HyperTabular::DelimitedReader.new(csv, HyperTabular::Dialect::CSV,
                                           [column.i32(0), column.decimal(1), column.text(2)])
check.("header", reader.header, %w[id amount name])
batch = reader.read
check.("rows and lines", [batch.rows, (0...batch.rows).map { |row| batch.line(row) }], [3, [2, 3, 4]])
check.("i32 column", batch.values(0), [1, 2, nil])
check.("decimal column", batch.values(1).map(&:to_r), [Rational(2469, 2), 7, 0])
check.("text, quotes resolved", batch.values(2), ["alice", "y\"z", "é"])
check.("fault span in characters", batch.get(0, 2),
       HyperCast::Fault.new(reason: :malformed, offset: 1, length: 1))
check.("raw text of a fault", batch.raw(0, 2), "1€")
check.("end of input", reader.read, nil)
broken = HyperTabular::DelimitedReader.new("a\n\"open\n", HyperTabular::Dialect::CSV, [column.text(0)])
kind = begin
  broken.read
rescue HyperTabular::TabularError => e
  e.kind
end
check.("structural failure", kind, :unclosed_quote)

# corpus/workbook/basic.ods, a LibreOffice workbook of two sheets.
ods = <<~B64.unpack1("m")
  UEsDBBQAAAAAAAAAIQCFbDmKLgAAAC4AAAAIAAAAbWltZXR5cGVhcHBsaWNhdGlvbi92bmQub2FzaXMub3BlbmRvY3VtZW50LnNw
  cmVhZHNoZWV0UEsDBBQAAAAIAOqRHF1DZdHovAAAAIwBAAAVAAAATUVUQS1JTkYvbWFuaWZlc3QueG1slZDBCsIwDIbvPsXIfavi
  RcqqN59AH6C0mSu0aVkzmW8vG7hNZAdv+ZM/+T9SX4bgiyd22UVScKj2UCCZaB09FNxv1/IEl/OuDppcg5nlpyiG4CnPUkHfkYw6
  uyxJB8ySjYwJyUbTBySW3345Jc1qBXCEdVzjPJZI3L0Wc9N7XybNrQKxdWNpB7ROl/xKqECn5J3R7CKJJ9lq4q3WmFVOHWqbW0QG
  8QeJicTj/hD8RjjjwGIcj2fFz0PPuzdQSwMEFAAAAAgA6pEcXR362fbPAgAA2A4AAAsAAABjb250ZW50LnhtbLVXTXObMBC951dQ
  tc0NW4A9SYghl7TTHjLTaZzeZbEQZoTESCKx/30HMBYQj41jcwG0q31vP8SyLB7WGbPeQKpU8AA5E4ws4FREKU8C9LL8ad+ih/Bq
  IeI4peBHghYZcG1TwTVwba0zxpVfawNUSO4LolLlc5KB8jX1RQ68sfLbu/2Kq5ZosmKDzavNHWtY68HGsNZtW0oYNeYy2UUYi4JH
  RKeCb/FgnYNMSxVhlZnfQWiDRpK8D3Wo3JvypDbfZqZVDg+Z5K9EtAmbhcolkEi9AujwalHnpLpa9XPJGaBHognq6m0qWJHxZluR
  rUBuZcqWkAPREAXIwe4MTXumr0AikLYU76qnkeK9TwOM7QIirABbb3IIkNIy5QkKF1Up8jCNFtPt42LaB/gkJslEwfXlcXM6Amgm
  OGwuDxuREUCVJll+eVhgJFcwwkGIGUkuj1reRkiClIdBO7L6heuILvF2xkwQjTqaADnGSef8uPdSuJO5IXEn8/NpcpC0bNYJ9Lnw
  xG2Tzb+fT0YLKYHTzY6qEQTo5fmx74B9Y+jtbzcTjM/3ICLaBFou7Caz2PVs7NlOO+adbFzimY0d23OWDvY97GNsPFifT6zTzBCX
  i4b4z9Kd//LwE8bP7TLXLpzPuxKCAeE76u26YdeyAEO7/PvyY4TvW/2grOpGA+ShabiqhZqspiE1nE3nBip4ZF2TLL+3rpm+ZymH
  60Tfm51bYsK50NXkY77RCRey05/37B0UXC2IhcwKRgIkYj9wphgdCNpqBqy2DqQU0uTj6+Pvf1P85eT22ZN0Z6Kyj7YGIvfDHHVK
  f7szzt6deB6ODGrehyntyClqVPWyObbqDfXLbZUD50j9foaOxeUaf2ajFtbBs9v5Dd5b3qFD8pFP84EJ/bl6L/vkJcSnOoPg7GDJ
  BiRrGPH+qraL5p7qRy9jTY9p/+nshPW/ULPq/5aGV/8BUEsBAhQDFAAAAAAAAAAhAIVsOYouAAAALgAAAAgAAAAAAAAAAAAAAIAB
  AAAAAG1pbWV0eXBlUEsBAhQDFAAAAAgA6pEcXUNl0ei8AAAAjAEAABUAAAAAAAAAAAAAAIABVAAAAE1FVEEtSU5GL21hbmlmZXN0
  LnhtbFBLAQIUAxQAAAAIAOqRHF0d+tn2zwIAANgOAAALAAAAAAAAAAAAAACAAUMBAABjb250ZW50LnhtbFBLBQYAAAAAAwADALIA
  AAA7BAAAAAA=
B64
book = HyperTabular::Workbook.new(ods)
check.("workbook format", [book.format, book.date_system], %i[ods y1900])
check.("sheets", book.sheets.map(&:name), %w[Data Second])
sheet = book.sheet("Data", HyperTabular::SheetOptions::DEFAULT, [column.i64(0), column.decimal(2)])
check.("sheet header", sheet.header.first(3), %w[id amount pct])
rows = sheet.read
check.("sheet values", [rows.values(0), rows.values(1).map(&:to_s)], [[1, 9, 9], ["0.25", "", ""]])
checks.join("\n")
