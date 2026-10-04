# Replays the shared conformance corpus (corpus/delimited.json at the repository root) —
# the same file the Rust and .NET bindings replay — through this binding: from a String in
# memory, from an IO read through buffers too small for a record, and from a file, in
# batches of one row, two rows and many. How the input is cut up is the binding's business
# and must not change the answer.
#
# The corpus pins fault spans as byte offsets into the cell's text; this binding presents
# them in the units String#[] slices by, as HyperCast's gem does, so each pinned span is
# mapped through `byteslice` before comparing — an identity on ASCII text.

require "json"
require "stringio"
require "tempfile"
require "spec_helper"

RSpec.describe "delimited conformance corpus" do
  # An IO with nothing but #read: the path a source without #readpartial takes.
  class ReadOnly
    def initialize(text)
      @io = StringIO.new(text)
    end

    def read(length)
      @io.read(length)
    end
  end

  def self.corpus(name)
    dir = __dir__
    until File.dirname(dir) == dir
      candidate = File.join(dir, "corpus", name)
      return JSON.parse(File.read(candidate, encoding: Encoding::UTF_8)) if File.exist?(candidate)

      dir = File.dirname(dir)
    end
    raise "corpus directory not found above #{__dir__}"
  end

  CORPUS = corpus("delimited.json")
  BATCH_ROWS = [1, 2, 1024].freeze
  BUFFER_BYTES = [1, 5, 64, HyperTabular::DelimitedReader::DEFAULT_BUFFER_BYTES].freeze

  def format_of(entry)
    spec = entry["format"]
    return HyperCast::NumFormat::INVARIANT unless spec

    HyperCast::NumFormat.new(decimal_sep: spec["decimal_sep"], group_sep: spec["group_sep"],
                             flags: spec["flags"], currency: spec.fetch("currency", ""))
  end

  def column_of(entry)
    ordinal = entry["ordinal"]
    door = entry["door"].to_sym
    case door
    when :unix then HyperTabular::Column.unix(ordinal, HyperCast::UNIX_PRECISIONS.key(entry["precision"]))
    when :excel_serial then HyperTabular::Column.excel_serial(ordinal, HyperCast::EXCEL_EPOCHS.key(entry["epoch"]))
    when :date_ordered, :datetime
      HyperTabular::Column.public_send(door, ordinal, HyperCast::DATE_ORDERS.key(entry["order"]))
    when *HyperTabular::Column::NUMERIC then HyperTabular::Column.public_send(door, ordinal, format_of(entry))
    else HyperTabular::Column.public_send(door, ordinal)
    end
  end

  # What the corpus says a cell that cast holds, as the Ruby value this binding owes for
  # the door: the type HyperCast's gem returns from the same one.
  def expected_value(door, cell)
    case door
    when :f32 then [cell["value"]].pack("e").unpack1("e")
    when :f64 then cell["value"].to_f
    when :decimal
      HyperCast::Decimal.new(magnitude: Integer(cell["magnitude"]), scale: cell["scale"], negative: cell["negative"])
    when :uuid then cell["value"].unpack("a8a4a4a4a12").join("-")
    when :timestamp, :unix, :excel_serial then Time.at(cell["seconds"], cell["nanos"], :nanosecond, in: "UTC")
    when :date, :date_ordered then Date.new(cell["year"], cell["month"], cell["day"])
    when :datetime
      second_of_day, nanos = cell["nanos_of_day"].divmod(1_000_000_000)
      DateTime.new(cell["year"], cell["month"], cell["day"], second_of_day / 3600, second_of_day / 60 % 60,
                   second_of_day % 60 + Rational(nanos, 1_000_000_000))
    when :time then cell["nanos"]
    when :duration then Rational(cell["seconds"] * 1_000_000_000 + cell["nanos"], 1_000_000_000)
    when :text then cell["text"]
    else cell["value"]
    end
  end

  # Holds one cell of the batch in hand to what the corpus says of it.
  def assert_cell(label, reader, column, row, expected)
    door = reader.column(column).door
    raw = reader.raw(column, row)
    expect(raw.encoding).to eq(Encoding::UTF_8), label
    # The union consumption idiom in action — pattern matching over the two case types.
    case reader.verdict(column, row)
    in HyperCast::Success(value:)
      expect(expected["expect"]).to eq("ok"), "#{label}: unexpectedly cast to #{value.inspect}"
      wanted = expected_value(door, expected)
      expect(value).to eq(wanted), "#{label}: #{value.inspect}, want #{wanted.inspect}"
      expect(value.class).to eq(wanted.class), label
      expect(value.to_s).to eq(expected["value"]), label if door == :decimal
      expect([value.utc?, value.nsec]).to eq([true, expected["nanos"]]), label if value.is_a?(Time)
      expect([value, value.encoding]).to eq([raw, Encoding::UTF_8]), label if door == :text
      expect(reader.values(column)[row]).to equal(value), label
    in HyperCast::Fault(reason:, offset:, length:)
      expect(reason.to_s).to eq(expected["expect"]), "#{label}: faulted #{reason} on #{raw.inspect}"
      expect(reader.values(column)[row]).to be_nil, label
      if (span = expected["fault"])
        # The cell's own text is still to hand, for the diagnostic a fault deserves.
        expect(raw).to eq(expected["raw"]), label
        expect([offset, length])
          .to eq([raw.byteslice(0, span[0]).length, raw.byteslice(span[0], span[1]).length]), label
      else
        expect([offset, length]).to eq([0, 0]), label
      end
    end
  end

  def assert_failure(label, actual, vector)
    expected = vector["failure"]
    if expected.nil?
      expect(actual).to be_nil, "#{label}: #{actual&.message}"
      return
    end
    expect(actual).to be_a(HyperTabular::TabularError), "#{label}: no failure"
    expect([actual.kind.to_s, actual.record, actual.line, actual.byte])
      .to eq(expected.values_at("kind", "record", "line", "byte")), label
    return unless expected["kind"] == "column_count"

    expect([actual.expected, actual.found]).to eq(expected.values_at("expected", "found")), label
  end

  def replay(label, vector, batch_rows)
    settings = vector["dialect"]
    dialect = HyperTabular::Dialect.new(
      separator: settings["separator"], quoting: settings["quoting"],
      has_header: settings["has_header"], skip_blank_lines: settings["skip_blank_lines"]
    )
    plan = vector["plan"].map { |entry| column_of(entry) }
    rows = vector["rows"]

    seen = 0
    failure = nil
    reader = nil
    begin
      reader = yield dialect, plan
      expect(reader.batch_rows).to eq(batch_rows), label
      expect(reader.header).to eq(vector["header"]), label
      while reader.read
        expect(reader.rows).to be_between(1, batch_rows), label
        # A cell at a time first, then the column at once: the two must tell one story.
        single = plan.each_index.map { |column| Array.new(reader.rows) { |row| reader.verdict(column, row) } }
        plan.each_index { |column| expect(reader.verdicts(column)).to eq(single[column]), label }
        reader.rows.times do |row|
          expect(seen).to be < rows.size, "#{label}: more rows than the corpus lists"
          plan.each_index do |column|
            assert_cell("#{label}, row #{seen}, column #{column}", reader, column, row, rows[seen][column])
          end
          seen += 1
        end
      end
      expect(reader.rows).to eq(0), label
    rescue HyperTabular::TabularError => e
      failure = e
      # A structural failure is final: the same one, again.
      expect { reader.read }.to raise_error(HyperTabular::TabularError) { |again| expect(again).to equal(e) } if reader
    ensure
      reader&.close
    end
    expect(seen).to eq(rows.size), label
    assert_failure(label, failure, vector)
  end

  it "has the cases, and through every door" do
    expect(CORPUS.size).to be >= 30
    doors = CORPUS.flat_map { |vector| vector["plan"].map { |entry| entry["door"].to_sym } }.uniq
    expect(doors).to match_array(HyperTabular::DOORS.keys)
  end

  CORPUS.each do |vector|
    name = vector["name"]

    it "replays \"#{name}\" every way it can be cut" do
      input = vector["input"]
      BATCH_ROWS.each do |batch_rows|
        replay("#{name} (memory, #{batch_rows} rows a batch)", vector, batch_rows) do |dialect, plan|
          HyperTabular::DelimitedReader.new(input, dialect, plan, batch_rows: batch_rows)
        end
        BUFFER_BYTES.each do |buffer_bytes|
          label = "#{name} (stream through #{buffer_bytes} bytes, #{batch_rows} rows a batch)"
          replay(label, vector, batch_rows) do |dialect, plan|
            HyperTabular::DelimitedReader.new(StringIO.new(input), dialect, plan,
                                              batch_rows: batch_rows, buffer_bytes: buffer_bytes)
          end
        end
        replay("#{name} (#read only, 3 bytes, #{batch_rows} rows a batch)", vector, batch_rows) do |dialect, plan|
          HyperTabular::DelimitedReader.new(ReadOnly.new(input), dialect, plan,
                                            batch_rows: batch_rows, buffer_bytes: 3)
        end
      end

      Tempfile.create(["corpus", ".csv"]) do |file|
        file.binmode
        file.write(input)
        file.close
        replay("#{name} (file, 7 bytes, 2 rows a batch)", vector, 2) do |dialect, plan|
          HyperTabular::DelimitedReader.open(file.path, dialect, plan, batch_rows: 2, buffer_bytes: 7)
        end
      end
    end
  end
end
