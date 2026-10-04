require "open3"
require "pathname"
require "stringio"
require "tempfile"
require "spec_helper"

RSpec.describe HyperTabular do
  csv = HyperTabular::Dialect::CSV
  column = HyperTabular::Column
  reader_class = HyperTabular::DelimitedReader

  def success(value) = HyperCast::Success.new(value: value)

  def fault(reason, offset = 0, length = 0) = HyperCast::Fault.new(reason: reason, offset: offset, length: length)

  # Whether HyperCast's own doors can be asked what they make of a text. The corpus is the
  # contract either way; where the doors are to hand, a few specs ask them directly too.
  def judge = HyperCast.available?

  # Every verdict of every remaining row, row by row.
  def drain(reader) = reader.each_row.to_a

  # Runs +script+ in a fresh interpreter with this tree's lib on the load path. From a
  # file, not -e: no platform's command line gets to reinterpret its quotes.
  def ruby(script, env = {})
    lib = File.expand_path("../lib", __dir__)
    Tempfile.create(["script", ".rb"]) do |file|
      file.write(script)
      file.close
      out, status = Open3.capture2(env, RbConfig.ruby, "-I", lib, "-r", "hypertabular", file.path)
      raise "subprocess failed: #{out}" unless status.success?

      out
    end
  end

  describe "the native core" do
    it "reports the loaded core's version, pinned to this gem's own" do
      expect(described_class.native_version).to match(/\A\d+\.\d+\.\d+\z/)
      expect(described_class.native_version).to eq(HyperTabular::VERSION)
    end

    it "answers available? without raising, and consistently with native_version" do
      expect(described_class.available?).to be(true)
      expect(described_class.available?).to be(true) # cached
    end

    it "runs on Fiddle, and accepts HYPERTABULAR_PURE whether or not it is set" do
      expect(HyperTabular::BACKEND).to eq(:fiddle)
      script = 'r = HyperTabular::DelimitedReader.new("a\n7\n", HyperTabular::Dialect::CSV, ' \
               "[HyperTabular::Column.u8(0)]); r.read; print HyperTabular::BACKEND, ' ', r.values(0).inspect"
      expect(ruby(script, "HYPERTABULAR_PURE" => "1")).to eq("fiddle [7]")
      expect(ruby(script, "HYPERTABULAR_PURE" => nil)).to eq("fiddle [7]")
    end

    it "answers available? false without raising when no library resolves, while a reader still raises" do
      script = "HyperTabular::Runtime.singleton_class.define_method(:library_path) { nil }; " \
               "print HyperTabular.available?; print ' '; " \
               'begin; HyperTabular::DelimitedReader.new("a\n", HyperTabular::Dialect::CSV, []); ' \
               "rescue LoadError => e; print e.message[/not found/]; end"
      expect(ruby(script)).to eq("false not found")
    end

    it "takes its verdict types from HyperCast's gem rather than carrying copies" do
      reader = reader_class.new("a\n1\nx\n", csv, [column.i32(0)])
      reader.read
      expect(reader.verdicts(0).map(&:class)).to eq([HyperCast::Success, HyperCast::Fault])
      expect(HyperTabular.constants).not_to include(:Success, :Fault, :NumFormat, :Decimal)
    end
  end

  describe HyperTabular::Dialect do
    it "defaults to what CSV means, and names the usual three" do
      expect(described_class.new(separator: ","))
        .to eq(described_class.new(separator: ",", quoting: true, has_header: true, skip_blank_lines: true))
      expect([described_class::CSV.separator, described_class::TSV.separator, described_class::PSV.separator])
        .to eq([",", "\t", "|"])
      expect(described_class::CSV).to be_frozen
    end

    it "packs to the core's four bytes" do
      expect(described_class.new(separator: ";", quoting: false, has_header: true, skip_blank_lines: true).packed)
        .to eq([0x3B, 0, 1, 0].pack("C4"))
    end

    it "refuses a separator that is not one byte, as the caller bug it is" do
      ["", ",,", "é", nil, 44].each do |separator|
        expect { described_class.new(separator: separator) }.to raise_error(ArgumentError, /one-byte String/)
      end
    end

    it "leaves which bytes the scanner can honour to the core, which says so when a reader is built" do
      ['"', "\n", "\r", "\0", "\x7F", "\xFF".b].each do |separator|
        dialect = described_class.new(separator: separator)
        expect { HyperTabular::DelimitedReader.new("a\n", dialect, []) }
          .to raise_error(ArgumentError, /not tab or printable ASCII/)
      end
    end
  end

  describe HyperTabular::Column do
    eurozone = HyperCast::NumFormat.new(decimal_sep: ",", group_sep: ".", flags: HyperCast::ALL_STYLES)

    it "has a factory for every door" do
      columns = {
        bool: described_class.bool(0),
        i8: described_class.i8(0), i16: described_class.i16(0), i32: described_class.i32(0),
        i64: described_class.i64(0), u8: described_class.u8(0), u16: described_class.u16(0),
        u32: described_class.u32(0), u64: described_class.u64(0), f32: described_class.f32(0),
        f64: described_class.f64(0), decimal: described_class.decimal(0),
        uuid: described_class.uuid(0), timestamp: described_class.timestamp(0),
        unix: described_class.unix(0, :seconds), excel_serial: described_class.excel_serial(0, :y1900),
        date: described_class.date(0), date_ordered: described_class.date_ordered(0, :day_month_year),
        datetime: described_class.datetime(0, :month_day_year), time: described_class.time(0),
        duration: described_class.duration(0), text: described_class.text(0)
      }
      expect(columns.keys).to match_array(HyperTabular::DOORS.keys)
      columns.each do |door, built|
        expect(built.door).to eq(door)
        expect(built.packed.bytesize).to eq(44)
        expect(built.packed.unpack("L<3")).to eq([0, HyperTabular::DOORS.fetch(door), built.packed.unpack("L<3").last])
        expect(built.value_bytes).to be_between(1, 16)
      end
    end

    it "declares the invariant notation for a numeric door unless told another, and none for the rest" do
      expect(described_class.i32(3).format).to equal(HyperCast::NumFormat::INVARIANT)
      expect(described_class.decimal(3, eurozone).format).to equal(eurozone)
      expect(described_class.decimal(3, eurozone).packed[12, 32]).to eq(eurozone.packed)
      expect(described_class.text(3).format).to be_nil
      expect(described_class.text(3).packed[12, 32]).to eq("\0".b * 32)
    end

    it "numbers what a door declares as HyperCast numbers it" do
      expect(described_class.unix(1, :nanoseconds).packed.unpack("L<3")).to eq([1, 14, 4])
      expect(described_class.excel_serial(1, :y1904).packed.unpack("L<3")).to eq([1, 22, 2])
      expect(described_class.datetime(1, :day_month_year).packed.unpack("L<3")).to eq([1, 21, 3])
      expect(described_class.date(1, :month_day_year)).to eq(described_class.date_ordered(1, :month_day_year))
      expect(described_class.date(1).declared).to be_nil
    end

    it "raises HyperCast's own KeyError for an undeclared option, never a verdict" do
      expect { described_class.unix(0, :fortnights) }.to raise_error(KeyError)
      expect { described_class.excel_serial(0, nil) }.to raise_error(KeyError)
      expect { described_class.date(0, "month_day_year") }.to raise_error(KeyError)
      expect { described_class.datetime(0, 2) }.to raise_error(KeyError)
      expect { described_class.new(ordinal: 0, door: :varchar) }.to raise_error(KeyError)
    end

    it "refuses an ordinal, a format or a declaration that cannot be meant" do
      [-1, 1 << 31, 1.0, "0", nil].each do |ordinal|
        expect { described_class.i32(ordinal) }.to raise_error(ArgumentError, /ordinal/)
      end
      expect { described_class.i32(0, ",") }.to raise_error(ArgumentError, /NumFormat/)
      expect { described_class.new(ordinal: 0, door: :text, format: eurozone) }.to raise_error(ArgumentError)
      expect { described_class.new(ordinal: 0, door: :bool, declared: :seconds) }.to raise_error(ArgumentError)
    end
  end

  describe HyperTabular::DelimitedReader do
    it "reads a header, then batches of typed columns" do
      reader = described_class.new("id,name,score\n1,alice,2.5\n2,bob,3\n", csv,
                                   [column.i32(0), column.text(1), column.f64(2)])
      expect(reader.header).to eq(%w[id name score]).and be_frozen
      expect([reader.rows, reader.records, reader.column_count]).to eq([0, 1, 3])
      expect(reader.column(1)).to eq(column.text(1))
      expect(reader.plan).to eq([column.i32(0), column.text(1), column.f64(2)]).and be_frozen

      expect(reader.read).to be(true)
      expect([reader.rows, reader.records]).to eq([2, 3])
      expect(reader.values(0)).to eq([1, 2]).and be_frozen
      expect(reader.values(1)).to eq(%w[alice bob])
      expect(reader.values(2)).to eq([2.5, 3.0])
      expect(reader.verdicts(0)).to eq([success(1), success(2)]).and be_frozen
      expect(reader.verdict(1, 1)).to eq(success("bob"))

      expect(reader.read).to be(false)
      expect(reader.rows).to eq(0)
      expect(reader.values(0)).to eq([])
      expect(reader.verdicts(0)).to eq([])
      expect(reader.read).to be(false)
    end

    it "hands out a verdict per cell that Ruby's pattern matching reads" do
      reader = described_class.new("n\n12\n12x4\n\n256\n", csv.with(skip_blank_lines: false), [column.u8(0)])
      reader.read
      seen = reader.rows.times.map do |row|
        case reader.verdict(0, row)
        in HyperCast::Success(value:) then "got #{value}"
        in HyperCast::Fault(reason: :empty) then "nothing"
        in HyperCast::Fault(reason:, offset:, length:) then "#{reason} #{reader.raw(0, row)[offset, length].inspect}"
        end
      end
      expect(seen).to eq(["got 12", 'malformed "x"', "nothing", 'out_of_range "256"'])
      expect(reader.values(0)).to eq([12, nil, nil, nil])
      expect(reader.verdict(0, 2)).to equal(described_class::EMPTY)
      expect(HyperCast.optional(reader.verdict(0, 2))).to be_nil
    end

    it "gives each door the Ruby type HyperCast's gem gives it" do
      text = "t,3,-4,2.5,1234.5,6ba7b810-9dad-11d1-80b4-00c04fd430c8,2024-01-31T10:30:00.5Z,1700000000123," \
             "45292.75,2024-01-31,7/1/2026,1/7/2026 3:04:05.25 PM,15:04:05.5,PT1H30M,héllo\n"
      plan = [
        column.bool(0), column.u64(1), column.i8(2), column.f32(3), column.decimal(4), column.uuid(5),
        column.timestamp(6), column.unix(7, :milliseconds), column.excel_serial(8, :y1900), column.date(9),
        column.date(10, :day_month_year), column.datetime(11, :month_day_year), column.time(12),
        column.duration(13), column.text(14)
      ]
      reader = described_class.new(text, csv.with(has_header: false), plan)
      expect(reader.header).to be_nil
      reader.read
      values = plan.each_index.map { |index| reader.values(index).first }
      expect(values).to eq([
        true, 3, -4, 2.5, HyperCast::Decimal.new(magnitude: 12_345, scale: 1, negative: false),
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        Time.utc(2024, 1, 31, 10, 30, Rational(1, 2)), Time.at(1_700_000_000, 123, :millisecond, in: "UTC"),
        Time.utc(2024, 1, 1, 18), Date.new(2024, 1, 31), Date.new(2026, 1, 7),
        DateTime.new(2026, 1, 7, 15, 4, Rational(21, 4)), 54_245_500_000_000, Rational(5400), "héllo"
      ])
      expect(values.map(&:class)).to eq([
        TrueClass, Integer, Integer, Float, HyperCast::Decimal, String, Time, Time, Time, Date, Date,
        DateTime, Integer, Rational, String
      ])
      expect(values[6..8].map(&:utc?)).to all(be(true))
      expect(values[11].sec_fraction).to eq(Rational(1, 4))
      expect(values[14].encoding).to eq(Encoding::UTF_8)
      next unless judge

      # The same answers HyperCast's own doors give for the same text.
      cells = text.chomp.split(",")
      invariant = HyperCast::NumFormat::INVARIANT
      expect(reader.verdict(4, 0)).to eq(HyperCast.decimal(cells[4], invariant))
      expect(reader.verdict(5, 0)).to eq(HyperCast.uuid(cells[5]))
      expect(reader.verdict(6, 0)).to eq(HyperCast.timestamp(cells[6]))
      expect(reader.verdict(8, 0)).to eq(HyperCast.excel_serial(cells[8], :y1900))
      expect(reader.verdict(11, 0)).to eq(HyperCast.datetime(cells[11], :month_day_year))
      expect(reader.verdict(13, 0)).to eq(HyperCast.duration(cells[13]))
    end

    it "keeps u64 and the 96-bit decimal whole" do
      reader = described_class.new("18446744073709551615,79228162514264337593543950335\n",
                                   csv.with(has_header: false), [column.u64(0), column.decimal(1)])
      reader.read
      expect(reader.values(0)).to eq([2**64 - 1])
      expect(reader.values(1).first.magnitude).to eq(2**96 - 1)
    end

    it "reads a column under the notation its plan declares" do
      eurozone = HyperCast::NumFormat.new(decimal_sep: ",", group_sep: ".", flags: HyperCast::ALL_STYLES)
      dollars = HyperCast::NumFormat.new(decimal_sep: ".", group_sep: ",", flags: HyperCast::ALL_STYLES, currency: "$")
      reader = described_class.new("1.234,5;($1,234.50)\n", csv.with(separator: ";", has_header: false),
                                   [column.f64(0, eurozone), column.decimal(1, dollars), column.f64(0)])
      reader.read
      expect(reader.values(0)).to eq([1234.5])
      expect(reader.values(1).map(&:to_s)).to eq(["-1234.5"])
      expect(reader.verdict(2, 0)).to eq(fault(:malformed, 5, 1))
      expect(reader.verdict(2, 0)).to eq(HyperCast.f64("1.234,5", HyperCast::NumFormat::INVARIANT)) if judge
    end

    it "projects: any source column, in any order, through more than one door, and past the end as empty" do
      reader = described_class.new("a,b,c\n1,x,2\n", csv, [column.text(2), column.i32(0), column.text(0), column.i32(7)])
      expect(drain(reader)).to eq([[success("2"), success(1), success("1"), fault(:empty)]])
    end

    it "counts rows through an empty plan" do
      reader = described_class.new("a\n1\n2\n3\n", csv, [], batch_rows: 2)
      expect([reader.read, reader.rows, reader.read, reader.rows, reader.read]).to eq([true, 2, true, 1, false])
    end

    it "speaks of a fault's span in the characters String#[] slices by, as HyperCast's gem does" do
      reader = described_class.new("n\né12x4\n\"é\"\"1x\"\n", csv, [column.i32(0), column.text(0)])
      reader.read
      # The core counts in bytes: é is two of them, and one character.
      expect(reader.verdicts(0)).to eq([fault(:malformed, 0, 1), fault(:malformed, 0, 1)])
      expect(reader.raw(0, 0)[0, 1]).to eq("é")
      # A cell that had to be unescaped is judged, and given back, unescaped.
      expect(reader.raw(0, 1)).to eq('é"1x')
      expect(reader.values(1)).to eq(["é12x4", 'é"1x'])

      late = described_class.new("n
1é2é3x
", csv, [column.i32(0)])
      late.read
      expect(late.verdict(0, 0)).to eq(fault(:malformed, 1, 1))
      expect(late.raw(0, 0).byteslice(1, 2)).to eq("é")

      # With characters ahead of it, the span's offset moves too: x is byte 5, character 3.
      euros = HyperCast::NumFormat.new(decimal_sep: ",", group_sep: ".", flags: HyperCast::ALL_STYLES, currency: "€")
      priced = described_class.new("n
€12x4
", csv, [column.i32(0, euros)])
      priced.read
      expect(priced.verdict(0, 0)).to eq(fault(:malformed, 3, 1))
      expect(priced.raw(0, 0)[3, 1]).to eq("x")
      next unless judge

      expect(reader.verdict(0, 0)).to eq(HyperCast.i32("é12x4", HyperCast::NumFormat::INVARIANT))
      expect(reader.verdict(0, 1)).to eq(HyperCast.i32('é"1x', HyperCast::NumFormat::INVARIANT))
      expect(late.verdict(0, 0)).to eq(HyperCast.i32("1é2é3x", HyperCast::NumFormat::INVARIANT))
      expect(priced.verdict(0, 0)).to eq(HyperCast.i32("€12x4", euros))
    end

    it "gives back the raw text of any cell, whatever its door and verdict" do
      reader = described_class.new("a,b\n\" 7 \",\"x\"\"y\"\n,\"\"\n", csv, [column.i32(0), column.text(1)])
      reader.read
      expect([reader.raw(0, 0), reader.raw(1, 0), reader.raw(0, 1), reader.raw(1, 1)]).to eq([" 7 ", 'x"y', "", ""])
      expect(reader.raw(0, 0)).to be_a(String).and have_attributes(encoding: Encoding::UTF_8)
      expect(reader.verdicts(1)).to eq([success('x"y'), fault(:empty)])
    end

    it "hands out values that outlive the batch they came from" do
      reader = described_class.new(StringIO.new("a\n\"x\"\"1\"\nplain-one\n\"y\"\"2\"\nplain-two\n"), csv,
                                   [column.text(0)], batch_rows: 2, buffer_bytes: 64)
      reader.read
      first = reader.values(0)
      raw = reader.raw(0, 0)
      reader.read
      expect(first).to eq(['x"1', "plain-one"])
      expect(raw).to eq('x"1')
      expect(reader.values(0)).to eq(['y"2', "plain-two"])
    end

    it "reads a String in place, immune to what the caller does to it next" do
      text = +"a,b\n1,one\n2,two\n"
      reader = described_class.new(text, csv, [column.i32(0), column.text(1)], batch_rows: 1)
      text.replace("x" * 64)
      text << "y"
      expect(drain(reader)).to eq([[success(1), success("one")], [success(2), success("two")]])

      frozen = "a\n5\n".freeze
      reader = described_class.new(frozen, csv, [column.i32(0)])
      expect(reader.instance_variable_get(:@buffer)).to equal(frozen)
      expect(drain(reader)).to eq([[success(5)]])
    end

    it "reads text in whatever encoding the String arrived in" do
      plan = [column.text(0), column.i32(1)]
      expected = [[success("é"), success(7)]]
      expect(drain(described_class.new("n,v\né,7\n".encode(Encoding::UTF_16LE), csv, plan))).to eq(expected)
      expect(drain(described_class.new("n,v\né,7\n".encode(Encoding::ISO_8859_1), csv, plan))).to eq(expected)
      binary = described_class.new("n,v\né,7\n".b, csv, plan)
      binary.read
      expect(binary.values(0)).to eq(["é"])
      expect(binary.values(0).first.encoding).to eq(Encoding::UTF_8)
      expect(binary.header).to eq(%w[n v])
    end

    it "skips a byte-order mark, however the input is cut" do
      [1, 2, 3, 4, 64].each do |buffer_bytes|
        reader = described_class.new(StringIO.new("﻿a,b\n1,2\n"), csv, [column.i32(1)],
                                     buffer_bytes: buffer_bytes)
        expect(reader.header).to eq(%w[a b])
        expect(drain(reader)).to eq([[success(2)]])
      end
    end

    it "streams many batches through a buffer smaller than the input" do
      rows = 10_000
      text = "id,name,score\n" + (0...rows).map { |i| "#{i},name #{i},#{i}.5\n" }.join
      [text, StringIO.new(text)].each do |source|
        reader = described_class.new(source, csv, [column.i64(0), column.text(1), column.f64(2)], buffer_bytes: 1000)
        batches = []
        ids = []
        names = []
        total = 0.0
        while reader.read
          batches << reader.rows
          ids.concat(reader.values(0))
          names.concat(reader.values(1))
          total += reader.values(2).sum
        end
        expect(batches.sum).to eq(rows)
        expect(batches.max).to be <= described_class::DEFAULT_BATCH_ROWS
        expect(ids).to eq((0...rows).to_a)
        expect([names.first, names.last]).to eq(["name 0", "name #{rows - 1}"])
        expect(total).to eq((0...rows).sum + rows * 0.5)
        expect(reader.records).to eq(rows + 1)
      end
    end

    it "grows the buffer for a record larger than it" do
      long = "x" * 5000
      reader = described_class.new(StringIO.new("a,b\n1,#{long}\n2,short\n"), csv, [column.i32(0), column.text(1)],
                                   buffer_bytes: 16)
      expect(drain(reader)).to eq([[success(1), success(long)], [success(2), success("short")]])
    end

    it "grows the arena for escaped text larger than it, in a header and in a batch" do
      long = 'say ""hi"" ' * 1000
      plain = 'say "hi" ' * 1000
      text = "\"#{long}\",b\n\"#{long}\",1\n\"#{long}\",2\n"
      [text, StringIO.new(text)].each do |source|
        reader = described_class.new(source, csv, [column.text(0), column.i32(1), column.text(0)], buffer_bytes: 100)
        expect(reader.header).to eq([plain, "b"])
        expect(drain(reader)).to eq([[success(plain), success(1), success(plain)],
                                     [success(plain), success(2), success(plain)]])
      end
    end

    it "reads a header wider than its first guess" do
      names = (0...300).map { |i| "c#{i}" }
      reader = described_class.new("#{names.join(',')}\n#{(0...300).to_a.join(',')}\n", csv,
                                   [column.i32(299), column.i32(0)])
      expect(reader.header).to eq(names)
      expect(drain(reader)).to eq([[success(299), success(0)]])
    end

    it "is empty, with an empty header, for an input with no record at all" do
      ["", "﻿", "\n\n"].each do |text|
        [text, StringIO.new(text)].each do |source|
          reader = described_class.new(source, csv, [column.i32(0)])
          expect(reader.header).to eq([])
          expect(reader.read).to be(false)
        end
      end
    end

    it "delivers every intact row, then raises the structural failure — the same one, every time" do
      reader = described_class.new("a,b\n1,2\n3\n4,5\n", csv, [column.i32(0)], batch_rows: 1)
      expect(reader.read).to be(true)
      expect(reader.values(0)).to eq([1])
      error = nil
      expect { reader.read }.to raise_error(HyperTabular::TabularError) { |e| error = e }
      expect([error.kind, error.record, error.line, error.byte, error.expected, error.found])
        .to eq([:column_count, 2, 3, 8, 2, 1])
      expect(error.message).to eq("Record 2 (line 3, byte 8) has 1 cells; the first record had 2.")
      expect(error.deconstruct_keys(nil))
        .to eq(kind: :column_count, record: 2, line: 3, byte: 8, expected: 2, found: 1)
      matched =
        case error
        in { kind: :column_count, line:, found: } then [line, found]
        end
      expect(matched).to eq([3, 1])
      expect(reader.rows).to eq(0)
      expect { reader.read }.to raise_error(HyperTabular::TabularError) { |again| expect(again).to equal(error) }
      expect { reader.each_row.to_a }.to raise_error(HyperTabular::TabularError)
    end

    it "raises a quote never closed, from the read that reaches it or from the header" do
      reader = described_class.new("a\n1\n\"2\n", csv, [column.i32(0)])
      expect(reader.read).to be(true)
      expect { reader.read }.to raise_error(HyperTabular::TabularError) { |e|
        expect([e.kind, e.record, e.line, e.byte, e.expected, e.found]).to eq([:unclosed_quote, 2, 3, 4, 0, 0])
        expect(e.message).to eq("The input ended inside a quoted cell in record 2 (line 3, byte 4).")
      }
      expect { described_class.new("\"a,b\n1,2\n", csv, [column.i32(0)]) }
        .to raise_error(HyperTabular::TabularError) { |e| expect([e.kind, e.record, e.line]).to eq([:unclosed_quote, 0, 1]) }
    end

    it "raises a record that outgrows the row ceiling, in memory and from a stream" do
      stub_const("HyperTabular::DelimitedReader::MAX_ROW_BYTES", 16)
      text = "a,b\n1,2\n3,#{'x' * 40}\n"
      [text, StringIO.new(text)].each do |source|
        reader = described_class.new(source, csv, [column.i32(0)], buffer_bytes: 8)
        expect(reader.read).to be(true)
        expect(reader.values(0)).to eq([1])
        error = nil
        expect { reader.read }.to raise_error(HyperTabular::TabularError) { |e| error = e }
        expect([error.kind, error.record, error.line, error.byte]).to eq([:row_too_long, 2, 3, 8])
        expect(error.message).to eq("Record 2 (line 3, byte 8) exceeds the 16-byte row ceiling.")
        expect { reader.read }.to raise_error(HyperTabular::TabularError) { |again| expect(again).to equal(error) }
      end
    end

    it "walks every remaining row with each_row, with or without a block" do
      reader = described_class.new("a,b\n1,x\n2,\n3,z\n", csv, [column.i32(0), column.text(1)], batch_rows: 2)
      rows = []
      expect(reader.each_row { |row| rows << row }).to equal(reader)
      expect(rows).to eq([[success(1), success("x")], [success(2), fault(:empty)], [success(3), success("z")]])
      expect(rows).to all(be_frozen)

      lazy = described_class.new("a\n1\n2\n", csv, [column.i32(0)]).each_row
      expect(lazy).to be_an(Enumerator)
      expect(lazy.map { |(verdict)| verdict.value }).to eq([1, 2])
    end

    it "opens a path, and closes the file with the block or with the reader" do
      Tempfile.create(["reader", ".tsv"]) do |file|
        file.binmode
        file.write("id\tname\r\n1\talice\r\n2\tbob\r\n")
        file.close
        plan = [column.i32(0), column.text(1)]

        result = described_class.open(file.path, HyperTabular::Dialect::TSV, plan) do |reader|
          expect(reader.header).to eq(%w[id name])
          [drain(reader), reader]
        end
        expect(result.first).to eq([[success(1), success("alice")], [success(2), success("bob")]])
        expect(result.last).to be_closed

        reader = described_class.open(Pathname(file.path), HyperTabular::Dialect::TSV, plan, batch_rows: 1)
        handle = reader.instance_variable_get(:@io)
        expect(reader.read).to be(true)
        expect(reader.close).to be_nil
        expect(handle).to be_closed
        expect(reader.close).to be_nil
      end
      expect { described_class.open("/no/such/file.csv", csv, []) }.to raise_error(Errno::ENOENT)
    end

    it "closes the file it opened when the header is broken" do
      Tempfile.create(["broken", ".csv"]) do |file|
        file.write("\"a,b\n")
        file.close
        opened = nil
        allow(File).to receive(:open).and_wrap_original { |original, *args| opened = original.call(*args) }
        expect { described_class.open(file.path, csv, []) }.to raise_error(HyperTabular::TabularError)
        expect(opened).to be_closed
      end
    end

    it "leaves an IO it was handed open, unless told to close it" do
      io = StringIO.new("a\n1\n")
      reader = described_class.new(io, csv, [column.i32(0)])
      reader.close
      expect(io).not_to be_closed
      expect(reader).to be_closed
      expect { reader.read }.to raise_error(IOError, /closed/)
      expect(reader.rows).to eq(0)

      described_class.new(io, csv, [column.i32(0)], close_source: true).close
      expect(io).to be_closed
    end

    it "raises IndexError for a cell outside the batch or the plan" do
      reader = described_class.new("a\n1\n2\n", csv, [column.i32(0)])
      expect { reader.verdict(0, 0) }.to raise_error(IndexError, /0 rows/)
      expect { reader.raw(0, 0) }.to raise_error(IndexError)
      reader.read
      [2, -1, 1.0, nil].each do |row|
        expect { reader.verdict(0, row) }.to raise_error(IndexError, /outside the batch/)
        expect { reader.raw(0, row) }.to raise_error(IndexError, /outside the batch/)
      end
      expect { reader.values(1) }.to raise_error(IndexError)
      expect { reader.verdicts(1) }.to raise_error(IndexError)
      expect { reader.raw(1, 0) }.to raise_error(IndexError)
      expect { reader.column(1) }.to raise_error(IndexError)
    end

    it "refuses what cannot be a reader's arguments, as caller bugs" do
      plan = [column.i32(0)]
      expect { described_class.new(42, csv, plan) }.to raise_error(ArgumentError, /String or an IO/)
      expect { described_class.new(Pathname("x.csv"), csv, plan) }.to raise_error(ArgumentError, /open reads a path/)
      expect { described_class.new("a\n", ",", plan) }.to raise_error(ArgumentError, /Dialect/)
      expect { described_class.new("a\n", csv, [:i32]) }.to raise_error(ArgumentError, /plan column 0/)
      [0, -1, 1.5, nil].each do |count|
        expect { described_class.new("a\n", csv, plan, batch_rows: count) }.to raise_error(ArgumentError, /batch_rows/)
        expect { described_class.new(StringIO.new("a\n"), csv, plan, buffer_bytes: count) }
          .to raise_error(ArgumentError, /buffer_bytes/)
      end
    end

    it "describes itself in a line, not by its buffers" do
      reader = described_class.new("a\n#{'1' * 500}\n", csv, [column.text(0)])
      expect(reader.inspect).to eq("#<HyperTabular::DelimitedReader columns=1 rows=0 records=1>")
      reader.read
      reader.close
      expect(reader.inspect).to eq("#<HyperTabular::DelimitedReader columns=1 rows=0 records=2 closed>")
    end

    # The input is a Ruby String the core reads in place, so it must not move while a
    # reader holds it — and a String short enough to live inside its object slot is one
    # the garbage collector's compaction would otherwise be free to move.
    it "holds its input where it is across a compacting collection" do
      skip "this Ruby's GC does not compact" unless GC.respond_to?(:compact) && begin
        GC.compact
        true
      rescue NotImplementedError
        false
      end

      readers = Array.new(500) do |i|
        [i, described_class.new("a,b\n#{i},t#{i}\n#{i + 1},u#{i}\n", csv, [column.i32(0), column.text(1)],
                                batch_rows: 1)]
      end
      readers.each_slice(50) do |slice|
        GC.compact
        slice.each do |i, reader|
          reader.read
          expect([reader.values(0), reader.values(1), reader.raw(1, 0)]).to eq([[i], ["t#{i}"], "t#{i}"])
        end
      end
      GC.compact
      readers.each do |i, reader|
        reader.read
        expect([reader.values(0), reader.values(1)]).to eq([[i + 1], ["u#{i}"]])
      end
    end
  end
end
