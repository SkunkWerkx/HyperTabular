# The options a file past the plain case needs, each by every route a source takes, what a batch
# says of its cells, and how long that stays right.

require "tempfile"
require "stringio"
require "spec_helper"
require_relative "corpus_helpers"
require_relative "package_helpers"

RSpec.describe "a messier file" do
  column = HyperTabular::Column
  reader = HyperTabular::DelimitedReader
  dialect_class = HyperTabular::Dialect
  sheet_options = HyperTabular::SheetOptions
  workbook = HyperTabular::Workbook
  csv = dialect_class::CSV
  text_plan = [column.text(0)]
  workbooks = File.join(CorpusHelpers.directory, "workbook")

  # A source handing over a few bytes a read, with #read alone.
  dribble = Class.new do
    def initialize(bytes)
      @io = StringIO.new(bytes)
    end

    def read(length)
      @io.read([length, 3].min)
    end
  end

  # What a read says, its rows as verdicts and origins and its faults counted across it: a stream
  # may deliver the same rows in more batches.
  said = lambda do |source, dialect, plan|
    rows = []
    faults = []
    ended = nil
    begin
      held = source.respond_to?(:call) ? source.call : reader.new(source, dialect, plan)
      while (batch = held.read)
        faults.concat(batch.faults.map { |problem| [problem.row + rows.size, problem.reason, problem.detail] })
        batch.rows.times do |row|
          rows << plan.each_index.map { |at| [batch.get(at, row), batch.origin(at, row)] }
        end
      end
    rescue HyperTabular::TabularError => e
      ended = [e.kind, e.record, e.line, e.byte]
    end
    [rows, faults, ended]
  end

  # The same, from a String, a file, a StringIO and a dribble, all the same or it fails; the rows'
  # values and what it failed with.
  define_method(:every_way) do |bytes, dialect, plan|
    first = said.call(bytes, dialect, plan)
    Tempfile.create(["input", ".csv"]) do |file|
      file.binmode
      file.write(bytes)
      file.close
      File.open(file.path, "rb") { |io| expect(said.call(io, dialect, plan)).to eq(first) }
      expect(said.call(-> { reader.open(file.path, dialect, plan) }, dialect, plan)).to eq(first)
    end
    expect(said.call(StringIO.new(bytes), dialect, plan)).to eq(first)
    expect(said.call(dribble.new(bytes), dialect, plan)).to eq(first)
    rows = first[0].map { |row| row.map { |verdict, _| verdict.is_a?(HyperCast::Success) ? verdict.value : verdict.reason } }
    first.last ? rows + [first.last.first] : rows
  end

  options = {
    "skip" => ["title\na\n7\n", { skip: 1 }, [column.i32(0)], [[7]], [[:malformed], [7]]],
    "header_row" => ["x\na\n7\n", { header_row: 1 }, [column.i32(0)], [[7]], [[:malformed], [7]]],
    "short_rows" => ["a,b\n7\n", { short_rows: :pad }, [column.i32(0), column.i32(1)], [[7, :empty]], [:column_count]],
    "long_rows report" => ["a\n7,8\n", { long_rows: :report }, [column.i32(0)], [[7]], [:column_count]],
    "long_rows truncate" => ["a\n7,8\n", { long_rows: :truncate }, [column.i32(0)], [[7]], [:column_count]],
    "windows-1252" => ["a\n\x80\n".b, { encoding: :windows_1252 }, text_plan, [["€"]], [[:malformed]]],
    "latin-1" => ["a\n\xE9\n".b, { encoding: :latin1 }, text_plan, [["é"]], [[:malformed]]],
    "utf-16le" => ["a\n€\n".encode("UTF-16LE").b, { encoding: :utf16le }, text_plan, [["€"]], [[:malformed], ["\0"]]],
    "tokens" => ["a\nNA\n7\n", { tokens: ["NA"] }, [column.i32(0)], [[:empty], [7]], [[:malformed], [7]]]
  }

  options.each do |name, (bytes, declared, plan, with, without)|
    it "has #{name} do its own work, by every route" do
      expect(every_way(bytes, dialect_class.new(separator: ",", **declared), plan)).to eq(with)
      expect(every_way(bytes, csv, plan)).to eq(without)
    end
  end

  it "refuses in a dialect what it can't be told" do
    [{ skip: -1 }, { header_row: -1 }, { skip: 1.5 }, { skip: 1 << 32 }, { tokens: "NA" }, { encoding: :ebcdic },
     { short_rows: :stretch }, { long_rows: 99 }].each do |declared|
      expect { dialect_class.new(separator: ",", **declared) }.to raise_error(ArgumentError), declared.inspect
    end
  end

  it "has no header when the header row is past the input" do
    expect(reader.new("x\n", dialect_class.new(separator: ",", header_row: 5)).header).to eq([])
  end

  it "counts what was passed over in the lines a batch says" do
    batch = reader.new("title\nx\na\n7\n8\n", dialect_class.new(separator: ",", skip: 1, header_row: 1),
                       [column.i32(0)]).read
    expect(batch.rows.times.map { |row| batch.line(row) }).to eq([4, 5])
    expect(batch.values(0)).to eq([7, 8])
  end

  it "takes a column's own tokens in place of the dialect's" do
    text = "a,b,c\nNA,NA,-\n"
    dialect = dialect_class.new(separator: ",", tokens: ["NA"])
    batch = reader.new(text, dialect, [column.i32(0), column.i32(1).with_tokens(["-"]),
                                       column.i32(2).with_tokens(["-"])]).read
    expect([batch.origin(0, 0), batch.get(0, 0).reason]).to eq(%i[token empty])
    expect(batch.get(1, 0).reason).to eq(:malformed)
    expect(batch.origin(2, 0)).to eq(:token)
    # An empty list says this column has none, the dialect's included.
    batch = reader.new(text, dialect, [column.i32(0).with_tokens([]), column.i32(1)]).read
    expect([batch.get(0, 0).reason, batch.origin(0, 0), batch.origin(1, 0)]).to eq(%i[malformed cell token])
    # Not said is not none.
    expect(column.i32(0).tokens).to be_nil
    expect(column.i32(0).with_tokens([]).tokens).to eq([])
    expect { column.i32(0).with_tokens("NA") }.to raise_error(ArgumentError)
  end

  it "never takes a quoted cell for a token" do
    batch = reader.new(%(a,b\nNA,"NA"\n), dialect_class.new(separator: ",", tokens: ["NA"]),
                       [column.text(0), column.text(1)]).read
    expect([batch.origin(0, 0), batch.get(0, 0).reason]).to eq(%i[token empty])
    expect([batch.origin(1, 0), batch.get(1, 0)]).to eq([:cell, HyperCast::Success.new(value: "NA")])
  end

  it "reads a column's date options and month names" do
    compact = HyperCast::DateOptions.new(flags: HyperCast::DateOptions::COMPACT_DATE)
    text = "d\n20260207\n"
    expect(reader.new(text, csv, [column.date(0)]).read.get(0, 0).reason).to eq(:malformed)
    declared = column.date(0).with_date_options(compact)
    expect(reader.new(text, csv, [declared]).read.values(0)).to eq([Date.new(2026, 2, 7)])
    expect([declared.date_options, declared.months]).to eq([compact, nil])

    names = %w[janv fevr mars avr mai juin juil aout sept oct nov dec]
    months = HyperCast::MonthNames.new(entries: names.each_with_index.map { |name, at| [at + 1, name] })
    text = "d\n2-fevr-2026\n"
    plain = column.date_ordered(0, :day_month_year)
    expect(reader.new(text, csv, [plain]).read.get(0, 0).reason).to eq(:malformed)
    named = plain.with_date_options(HyperCast::DateOptions.new, months)
    expect(reader.new(text, csv, [named]).read.values(0)).to eq([Date.new(2026, 2, 2)])
    expect { column.date(0).with_date_options(:compact) }.to raise_error(ArgumentError)
    expect { column.date(0).with_date_options(compact, %w[jan]) }.to raise_error(ArgumentError)
  end

  it "says what failed on the fault and on the list" do
    batch = reader.new("a,b\n1x,300\n", csv, [column.i32(0), column.u8(1)]).read
    malformed = batch.get(0, 0)
    out_of_range = batch.get(1, 0)
    expect(malformed.detail).to eq(HyperCast::Detail::UNEXPECTED_CHARACTER)
    expect(out_of_range.reason).to eq(:out_of_range)
    first, second = batch.faults
    expect(first).to be_a(HyperTabular::Problem)
    expect(first.detail).to eq(malformed.detail)
    expect([second.detail, second.reason]).to eq([out_of_range.detail, :out_of_range])
    # The same by each way there is to a cell.
    expect(batch.verdicts(0)[0].detail).to eq(malformed.detail)
    expect(batch.first.get(0).detail).to eq(malformed.detail)
    expect(batch.first.get(0)).to match(satisfy do |fault|
      case fault
      in HyperCast::Fault(detail: HyperCast::Detail::UNEXPECTED_CHARACTER) then true
      else false
      end
    end)
  end

  it "counts a fault's span in characters on the verdict and the list, the core's bytes moved" do
    # Two two-byte digits, then the character the core stops at: byte 4, character 2.
    arabic = HyperCast::NumFormat.new(decimal_sep: ".", group_sep: ",", flags: HyperCast::ALL_STYLES, digit_zero: "٠")
    batch = reader.new("a\n٠٠x\n", csv, [column.i32(0, arabic)]).read
    verdict = batch.get(0, 0)
    problem = batch.faults.first
    expect([verdict.offset, verdict.length]).to eq([2, 1])
    expect([problem.offset, problem.length]).to eq([2, 1])
    expect(batch.raw(0, 0)[verdict.offset, verdict.length]).to eq("x")
  end

  it "says nothing of a fault but its reason with notes off" do
    plan = [column.i32(0)]
    ["a\n1x\n", StringIO.new("a\n1x\n")].each do |source|
      batch = reader.new(source, csv, plan, notes: false).read
      fault = batch.get(0, 0)
      expect([fault.reason, fault.detail]).to eq([:malformed, nil])
      problem = batch.faults.first
      expect([problem.detail, problem.origin, batch.origin(0, 0)]).to eq([nil, :cell, :cell])
    end
  end

  it "takes a token for an empty cell with notes off, and says no more" do
    batch = reader.new("a\nNA\n", dialect_class.new(separator: ",", tokens: ["NA"]), [column.i32(0)], notes: false).read
    expect([batch.get(0, 0).reason, batch.origin(0, 0), batch.faults]).to eq([:empty, :cell, []])
  end

  it "reads the same values with notes off" do
    text = "a,b\n1,x\n2,y\n"
    plan = [column.i32(0), column.text(1)]
    on = reader.new(text, csv, plan).read
    off = reader.new(text, csv, plan, notes: false).read
    expect([on.values(0), on.values(1)]).to eq([off.values(0), off.values(1)])
    expect([on.faults, off.faults]).to eq([[], []])
  end

  it "says a padded cell's a blank, and lists a long row" do
    dialect = dialect_class.new(separator: ",", short_rows: :pad, long_rows: :report)
    batch = reader.new("a,b\n1\n2,3,4\n5,6\n", dialect, [column.i32(0), column.i32(1)]).read
    expect([batch.origin(1, 0), batch.get(1, 0).reason]).to eq(%i[padded empty])
    long_row, = batch.faults
    expect(batch.faults.size).to eq(1)
    expect([long_row.row, long_row.plan_column, long_row.source_column]).to eq([1, nil, nil])
    expect([long_row.expected, long_row.found, long_row.line]).to eq([2, 3, 3])
    expect([long_row.reason, long_row.origin]).to eq([nil, :cell])
  end

  it "names a problem's source column as well as its plan's" do
    problem = reader.new("a,b,c\n1,2,x\n", csv, [column.i32(2)]).read.faults.first
    expect([problem.plan_column, problem.source_column, problem.offset, problem.length]).to eq([0, 2, 0, 1])
  end

  it "keeps a range a value" do
    range = HyperTabular::Range
    made = range.new(first_row: 4, first_column: 2, last_row: 12, last_column: 3)
    expect(made.to_h).to eq(first_row: 4, first_column: 2, last_row: 12, last_column: 3)
    expect(made).to eq(range.parse("B4:C12"))
    expect(made.hash).to eq(range.parse("B4:C12").hash)
    expect(made).not_to eq(range.parse("B4:C13"))
    expect(range.new).to eq(range.new(first_row: 0, first_column: 0, last_row: 0, last_column: 0))
    ["", "A", "1", "A1", "B4:", "C12:B4", "A0:B1"].each do |text|
      expect { range.parse(text) }.to raise_error(ArgumentError), text
    end
    expect { range.new(first_row: -1) }.to raise_error(ArgumentError)
    # 0 is no limit, so a first with no last is every row from there, and a last before it is refused.
    expect(range.new(first_row: 5).last_row).to eq(0)
    expect { range.new(first_row: 5, last_row: 4) }.to raise_error(ArgumentError)
  end

  it "takes a sheet's range, its plan counting from its first column and its faults saying where in the sheet" do
    book = workbook.open(File.join(workbooks, "ranges.xlsx"))
    whole = book.sheet(0, sheet_options::DEFAULT).suggest
    ranged = book.sheet(0, sheet_options.new(range: HyperTabular::Range.parse("B4:C12")))
    found = ranged.suggest
    expect(found.plan.size).to eq(2)
    expect(whole.plan.size).not_to eq(2)
    expect(found.report[0].name).to eq("id")
    expect(ranged.header).to eq(["id", found.report[1].name])
    plan = [column.i32(0), column.text(1)]
    listed = book.sheet(0, sheet_options.new(range: HyperTabular::Range.parse("B4:C12")),
                        plan).each_batch.flat_map do |batch|
      batch.faults.map { |problem| [problem.source_column, problem.plan_column, batch.line(problem.row) > 4] }
    end
    expect(listed).not_to be_empty
    expect(listed).to all(eq([1, 0, true]))
  end

  it "has a sheet's skip, header row and tokens change what it reads" do
    book = workbook.open(File.join(workbooks, "basic.xlsx"))
    plan = [column.text(1)]
    plain = book.sheet(0, sheet_options::DEFAULT, plan)
    expect(plain.header).not_to be_nil
    rows = plain.each_batch.flat_map { |batch| batch.values(0) }
    skipped = book.sheet(0, sheet_options.new(skip: 1), plan)
    expect(skipped.header[1]).to eq(rows[0])
    expect(skipped.each_batch.flat_map { |batch| batch.values(0) }).to eq(rows[1..])
    headed = book.sheet(0, sheet_options.new(header_row: 1), plan)
    expect(headed.header).to eq(skipped.header)
    expect(headed.each_batch.flat_map { |batch| batch.values(0) }).to eq(rows[1..])
    batch = book.sheet(0, sheet_options.new(tokens: [rows[0]]), plan).read
    expect([batch.get(0, 0).reason, batch.origin(0, 0)]).to eq(%i[empty token])
    # Notes off, a sheet has no origin to say.
    batch = book.sheet(0, sheet_options.new(tokens: [rows[0]], notes: false), plan).read
    expect([batch.get(0, 0).reason, batch.origin(0, 0)]).to eq(%i[empty cell])
    [{ skip: -1 }, { header_row: -1 }].each do |declared|
      expect { sheet_options.new(**declared) }.to raise_error(ArgumentError)
    end
  end

  it "keeps every row with skip_empty_rows off, a formula with no cached value among them" do
    book = workbook.open(File.join(workbooks, "uncached.xlsx"))
    plan = [column.text(0), column.i32(1)]
    kept = book.sheet(0, sheet_options.new(skip_empty_rows: false), plan).each_batch.to_a
    dropped = book.sheet(0, sheet_options.new(skip_empty_rows: true), plan).each_batch.to_a
    expect(kept.sum(&:rows)).to eq(dropped.sum(&:rows) + 2)
    expect(kept.flat_map { |batch| batch.rows.times.map { |row| batch.origin(1, row) } }).to include(:uncached)
    expect(kept.first.faults.map(&:origin)).to include(:uncached)
    # With notes off, the list can't say a formula has no cached value.
    batch = book.sheet(0, sheet_options.new(skip_empty_rows: false, notes: false), plan).read
    expect(batch.faults.map(&:origin)).not_to include(:uncached)
    expect(batch.origin(1, 0)).to eq(:cell)
  end

  it "reads a string longer than the window from a deflated sheet" do
    long = "x" * (256 * 1024)
    package = PackageHelpers.xlsx(%(<row r="1"><c t="inlineStr"><is><t>#{long}</t></is></c></row>))
    batch = workbook.new(package).sheet(0, sheet_options.new(has_header: false), [column.text(0)]).read
    expect(batch.values(0)).to eq([long])
    # And a shared one, every buffer starting at no room.
    package = PackageHelpers.xlsx(%(<row r="1"><c t="s"><v>0</v></c></row>), strings: "<si><t>#{long}</t></si>")
    HyperTabular::Runtime::Book.stingy = true
    begin
      batch = workbook.new(package).sheet(0, sheet_options.new(has_header: false), [column.text(0)]).read
      expect(batch.values(0)).to eq([long])
    ensure
      HyperTabular::Runtime::Book.stingy = false
    end
  end

  # A batch's last column is the last it asked for, in a range ending at it.
  it "ends a range's table at its last column" do
    book = workbook.open(File.join(workbooks, "ranges.xlsx"))
    sheet = book.sheet(0, sheet_options.new(range: HyperTabular::Range.parse("B4:C12")))
    expect(sheet.header.size).to eq(2)
    batch = sheet.bind([column.text(1), column.text(2)]).read
    expect(batch.values(1).uniq).to eq([nil])
  end

  it "says the ceiling of the decoded text, after what came before it" do
    { windows_1252: Encoding::Windows_1252, utf16le: Encoding::UTF_16LE }.each do |encoding, codec|
      stub_const("HyperTabular::DelimitedReader::MAX_ROW_BYTES", 100)
      dialect = dialect_class.new(separator: ",", encoding: encoding)
      bytes = "a\n1\n#{'x' * 500}\n".encode(codec).b
      [bytes, StringIO.new(bytes)].each do |source|
        held = reader.new(source, dialect, text_plan, batch_rows: 1, buffer_bytes: 8)
        expect(held.read.values(0)).to eq(["1"])
        expect { held.read }.to raise_error(HyperTabular::TabularError) { |e| expect(e.kind).to eq(:row_too_long) }
      end
    end
  end

  it "says an encoding failure after every intact row, and the file's byte" do
    dialect = dialect_class.new(separator: ",", encoding: :utf16le)
    bytes = "a\n1\n2\n".encode("UTF-16LE").b + "\x00\xDC\n\x00".b
    [bytes, StringIO.new(bytes), dribble.new(bytes)].each do |source|
      seen = []
      held = reader.new(source, dialect, [column.i32(0)])
      expect { held.each { |row| seen << row.value(0) } }
        .to raise_error(HyperTabular::TabularError) { |e|
          expect([e.kind, e.record, e.line, e.byte]).to eq([:encoding, 3, 4, 12])
        }
      expect(seen).to eq([1, 2])
      # Final: the same one, again.
      first = begin
        held.read
      rescue HyperTabular::TabularError => e
        e
      end
      expect { held.read }.to raise_error(HyperTabular::TabularError) { |e| expect(e).to equal(first) }
    end
  end

  it "finishes a held carriage return before an encoding failure says so" do
    bad = dialect_class.new(separator: ",", encoding: :utf16le)
    bytes = "a\n1\r".encode("UTF-16LE").b + "\x00\xDC".b
    seen = []
    held = reader.new(bytes, bad, [column.i32(0)])
    expect { held.each { |row| seen << row.value(0) } }
      .to raise_error(HyperTabular::TabularError) { |e| expect(e.kind).to eq(:encoding) }
    expect(seen).to eq([1])
  end

  it "says a failure in the header at opening, and of the file's byte" do
    dialect = dialect_class.new(separator: ",", encoding: :utf16le)
    bytes = "\xFF\xFEa\x00,\x00\x00\xD8".b
    [bytes, StringIO.new(bytes), dribble.new(bytes)].each do |source|
      expect { reader.new(source, dialect) }.to raise_error(HyperTabular::TabularError) { |e| expect(e.byte).to eq(6) }
    end
  end

  it "reads an encoded source by #read alone, by a mark and with a multi-byte character cut across reads" do
    dialect = dialect_class.new(separator: ",", encoding: :utf16le)
    bytes = "﻿a\n€\n🙂\n".encode("UTF-16LE").b
    held = reader.new(dribble.new(bytes), dialect, text_plan, buffer_bytes: 1)
    expect(held.header).to eq(["a"])
    expect(held.map { |row| row.value(0) }).to eq(["€", "🙂"])
  end

  # A stream that hands over one transfer and then has nothing until it's let go: what a pipe or a
  # socket does. The test fails if the call under test waits for it.
  stalling = Class.new do
    attr_reader :reads

    def initialize(*parts)
      @parts = parts
      @reads = 0
      @go = Queue.new
    end

    def read(_length)
      @reads += 1
      return @parts.shift unless @parts.empty?

      @go.pop(timeout: 30)
      nil
    end

    def release = @go << true
  end

  # +call+'s answer on a thread, or a failure if it takes five seconds, letting the stream go so
  # that the thread ends.
  define_method(:timed) do |stream, &call|
    worker = Thread.new(&call)
    answered = worker.join(5)
    stream.release unless answered
    value = worker.value
    expect(answered).not_to be_nil, "it waited on the stream with finished rows in hand"
    value
  end

  { utf8: "UTF-8", windows_1252: "Windows-1252", utf16le: "UTF-16LE" }.each do |encoding, codec|
    it "brings a stalled stream's finished rows to the caller, #{encoding}" do
      stream = stalling.new("a\n1\n2\n".encode(codec).b, "3\n".encode(codec).b)
      dialect = dialect_class.new(separator: ",", encoding: encoding)
      held = timed(stream) { reader.new(stream, dialect, [column.i32(0)]) }
      expect(held.header).to eq(["a"])
      expect(stream.reads).to eq(1)
      batch = timed(stream) { held.read }
      expect(batch.values(0).first(2)).to eq([1, 2])
      expect(stream.reads).to be <= 2
      seen = batch.rows
      while seen < 3
        batch = timed(stream) { held.read }
        seen += batch.rows
      end
      stream.release
      expect(timed(stream) { held.read }).to be_nil
    end

    it "brings the rows of the one transfer after a header's to the caller, #{encoding}" do
      stream = stalling.new("a\n".encode(codec).b, "1\n2\n".encode(codec).b)
      dialect = dialect_class.new(separator: ",", encoding: encoding)
      held = timed(stream) { reader.new(stream, dialect, [column.i32(0)]) }
      batch = timed(stream) { held.read }
      expect(batch.values(0)).to eq([1, 2])
      expect(stream.reads).to eq(2)
      stream.release
      timed(stream) { held.read }
    end
  end

  it "brings a pipe's finished rows to the caller" do
    readable, writable = IO.pipe
    writable.write("a\n1\n2\n")
    held = reader.new(readable, csv, [column.i32(0)])
    worker = Thread.new { held.read }
    answered = worker.join(5)
    writable.close
    expect(answered).not_to be_nil
    expect(worker.value.values(0)).to eq([1, 2])
    readable.close
  end

  # What a source raises goes to the caller and is final, as in Rust and Go: Ruby can't tell it from
  # an exception that landed in the read and took bytes with it.
  { utf8: "UTF-8", utf16le: "UTF-16LE" }.each do |encoding, codec|
    it "hands a stream's error to the caller and refuses every later read, #{encoding}" do
      source = Class.new do
        def initialize(parts) = @parts = parts

        def read(_length)
          part = @parts.shift
          raise IOError, "the wire went" if part == :raise

          part
        end
      end.new(["a\n1\n".encode(codec).b, :raise, "2\n".encode(codec).b, nil])
      dialect = dialect_class.new(separator: ",", encoding: encoding)
      held = reader.new(source, dialect, [column.i32(0)])
      expect(held.read.values(0)).to eq([1])
      expect { held.read }.to raise_error(IOError, "the wire went")
      refusal = /\Athe reader can't read on: IOError \(the wire went\)/
      2.times { expect { held.read }.to raise_error(RuntimeError, refusal) }
    end
  end

  # A record cut by the error, longer than the first record of what was read ahead of it: every row
  # finished before it is handed over before the source is asked again and raises.
  { utf8: "UTF-8", latin1: "ISO-8859-1", windows_1252: "Windows-1252", utf16le: "UTF-16LE" }.each do |encoding, codec|
    it "hands over every finished row before a stream raises with a record pending, #{encoding}" do
      source = Class.new do
        def initialize(parts) = @parts = parts

        def read(_length)
          part = @parts.shift
          raise IOError, "the wire went" if part == :raise

          part
        end
      end.new(["h\nr1\nr2\nr3\nr4\nlongpartial".encode(codec).b, :raise, :raise, "tail\nr6\n".encode(codec).b, nil])
      dialect = dialect_class.new(separator: ",", encoding: encoding)
      held = reader.new(source, dialect, [column.text(0)], batch_rows: 2)
      rows = []
      expect do
        while (batch = held.read)
          batch.rows.times { |row| rows << [batch.line(row), batch.values(0)[row]] }
        end
      end.to raise_error(IOError, "the wire went")
      expect(rows).to eq([[2, "r1"], [3, "r2"], [4, "r3"], [5, "r4"]])
      expect { held.read }.to raise_error(RuntimeError, /\Athe reader can't read on: IOError \(the wire went\)/)
    end
  end

  it "keeps a batch whole past its reader and the next read" do
    text = "a,b\n1x,y\n#{(0...3).map { |row| "#{row},z\n" }.join}2x,w\n"
    held = reader.new(text, csv, [column.i32(0), column.text(1)], batch_rows: 1)
    first = held.read
    3.times { held.read }
    held.close
    held = nil # rubocop:disable Lint/UselessAssignment
    GC.start
    problem = first.faults.first
    expect(first.get(0, 0).detail).to eq(HyperCast::Detail::UNEXPECTED_CHARACTER)
    expect([first.raw(0, 0), first.get(1, 0).value, first.origin(0, 0)]).to eq(["1x", "y", :cell])
    expect([problem.row, problem.line]).to eq([0, 2])
  end

  it "keeps a batch's list its own, frozen, and computes it only when asked" do
    batch = reader.new("a\nx\ny\n", csv, [column.i32(0)]).read
    expect(batch.faults.size).to eq(2)
    expect(batch.faults).to be_frozen
    expect(batch.faults).to equal(batch.faults)
    allow(HyperTabular::Runtime).to receive(:faults).and_call_original
    unasked = reader.new("a\nx\ny\n", csv, [column.i32(0)]).read
    unasked.values(0)
    unasked.get(0, 0)
    expect(HyperTabular::Runtime).not_to have_received(:faults)
    2.times { unasked.faults }
    expect(HyperTabular::Runtime).to have_received(:faults).once
  end

  it "answers for the batch asked, whichever is read since, a stateful column across batches included" do
    held = reader.new("a,b\n1,x\ny,2\n3,4\n", csv, [column.i32(0), column.i32(1)], batch_rows: 1)
    first = held.read
    second = held.read
    third = held.read
    expect(third.faults).to eq([])
    expect(second.faults.map { |problem| [problem.row, problem.line, problem.plan_column] }).to eq([[0, 3, 0]])
    expect(first.faults.map { |problem| [problem.row, problem.line, problem.plan_column] }).to eq([[0, 2, 1]])
  end

  it "keeps a sheet's batch whole past its sheet, with its notes" do
    book = workbook.open(File.join(workbooks, "ranges.xlsx"))
    batch = book.sheet(0, sheet_options::DEFAULT, [column.i32(1)]).read
    said = batch.faults
    book = nil # rubocop:disable Lint/UselessAssignment
    GC.start
    expect(batch.faults).to eq(said)
  end

  it "says so of a reader after close, and a batch still reads" do
    held = reader.new("a\nx\n", csv, [column.i32(0)])
    batch = held.read
    held.close
    expect { held.read }.to raise_error(IOError)
    expect(batch.faults.first.reason).to eq(:malformed)
  end

  it "takes notes: as its own keyword on every way to open" do
    Tempfile.create(["a", ".csv"]) do |file|
      file.write("a\nx\n")
      file.close
      batch = reader.open(file.path, csv, [column.i32(0)], notes: false).read
      expect(batch.get(0, 0).detail).to be_nil
    end
    expect { reader.open("/no/such.csv", csv, notes: false) }.to raise_error(Errno::ENOENT)
  end

  it "reads a lookup that a header declares twice by position, and first-match when asked" do
    held = reader.new("id,x,id\n1,2,3\n", csv)
    expect { held.header.ordinal("id") }.to raise_error(HyperTabular::AmbiguousColumn)
    expect(held.header.find_ordinal("id")).to eq(0)
    expect(held.header.ordinal("x")).to eq(1)
    held.bind([column.i32(2)])
    expect(held.read.values(0)).to eq([3])
  end
end
