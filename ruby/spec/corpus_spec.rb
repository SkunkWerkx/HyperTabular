# Replays the shared conformance corpus (corpus/delimited.json at the repository root) —
# the same file the Rust and .NET bindings replay — through this binding: from a String in
# memory, from an IO read through buffers too small for a record, and from a file, in
# batches of one row, two rows and many — each with its plan up front and header first,
# the plan bound once the header has been read. How the input is cut up, and when the plan
# is given, is the binding's business and must not change the answer.
#
# The corpus pins fault spans as byte offsets into the cell's text; this binding presents
# them in the units String#[] slices by, as HyperCast's gem does, so each pinned span is
# mapped through `byteslice` before comparing — an identity on ASCII text.

require "stringio"
require "tempfile"
require "spec_helper"
require_relative "corpus_helpers"

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

  CORPUS = CorpusHelpers.corpus("delimited.json")
  BATCH_ROWS = [1, 2, 1024].freeze
  BUFFER_BYTES = [1, 5, 64, HyperTabular::DelimitedReader::DEFAULT_BUFFER_BYTES].freeze

  include CorpusHelpers

  # A reader opened without a plan, held to what it is before its plan comes — unbound, an
  # empty plan, its header already read and counted — and then bound to +plan+.
  def header_first(vector, plan)
    reader = yield
    expect(reader).not_to be_bound
    expect(reader.plan).to eq([])
    expect(reader.header).to eq(vector["header"])
    expect(reader.column_count).to eq(reader.header&.size&.nonzero?)
    reader.bind(plan)
    expect(reader).to be_bound
    reader
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
      expect(reader.plan).to eq(plan), label
      while (batch = reader.read)
        expect(batch.rows).to be_between(1, batch_rows), label
        expect(batch.columns).to eq(plan), label
        # A cell at a time first, then the column at once: the two must tell one story.
        single = plan.each_index.map { |column| Array.new(batch.rows) { |row| batch.get(column, row) } }
        plan.each_index { |column| expect(batch.verdicts(column)).to eq(single[column]), label }
        batch.rows.times do |row|
          expect(seen).to be < rows.size, "#{label}: more rows than the corpus lists"
          plan.each_index do |column|
            assert_cell("#{label}, row #{seen}, column #{column}", batch, column, row, rows[seen][column])
          end
          seen += 1
        end
      end
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
        replay("#{name} (memory, header first, #{batch_rows} rows a batch)", vector, batch_rows) do |dialect, plan|
          header_first(vector, plan) { HyperTabular::DelimitedReader.new(input, dialect, batch_rows: batch_rows) }
        end
        label = "#{name} (stream through 5 bytes, header first, #{batch_rows} rows a batch)"
        replay(label, vector, batch_rows) do |dialect, plan|
          header_first(vector, plan) do
            HyperTabular::DelimitedReader.new(StringIO.new(input), dialect, batch_rows: batch_rows, buffer_bytes: 5)
          end
        end
      end

      Tempfile.create(["corpus", ".csv"]) do |file|
        file.binmode
        file.write(input)
        file.close
        replay("#{name} (file, 7 bytes, 2 rows a batch)", vector, 2) do |dialect, plan|
          HyperTabular::DelimitedReader.open(file.path, dialect, plan, batch_rows: 2, buffer_bytes: 7)
        end
        replay("#{name} (file, header first, 7 bytes, 2 rows a batch)", vector, 2) do |dialect, plan|
          header_first(vector, plan) do
            HyperTabular::DelimitedReader.open(file.path, dialect, batch_rows: 2, buffer_bytes: 7)
          end
        end
      end
    end
  end
end
