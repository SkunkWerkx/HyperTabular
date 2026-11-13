# Replays the shared conformance corpus (corpus/delimited.json at the repository root) —
# the same file the Rust and .NET bindings replay — through this binding: from a String in
# memory, from an IO read through buffers too small for a record, and from a file, in
# batches of one row, two rows and many — each with its plan up front and header first,
# the plan bound once the header has been read. How the input is cut up, and when the plan
# is given, is the binding's business and must not change the answer. Each batch's faults list
# is held to the case's problems, a case's suggest to what a suggestion says, and a key this
# replay doesn't know fails it.
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

  def replay(label, vector, batch_rows, notes: true)
    dialect = dialect_of(vector["dialect"])
    plan = vector["plan"].map { |entry| column_of(entry) }
    rows = vector["rows"]

    seen = 0
    failure = nil
    problems = []
    reader = nil
    begin
      reader = yield dialect, plan
      expect(reader.batch_rows).to eq(batch_rows), label
      expect(reader.header).to eq(vector["header"]), label
      expect(reader.plan).to eq(plan), label
      while (batch = reader.read)
        expect(batch.rows).to be_between(1, batch_rows), label
        expect(batch.columns).to eq(plan), label
        problems.concat(batch.faults.map { |problem| problem_json(batch, problem, seen) })
        assert_rows(label, plan, batch)
        # A cell at a time first, then the column at once: the two must tell one story.
        single = plan.each_index.map do |column|
          Array.new(batch.rows) { |row| batch.send(:verdict_of, column, row) }
        end
        plan.each_index do |column|
          if plan[column].checked?
            expect { batch.verdicts(column) }.to raise_error(TypeError, /get_checked/), label
          else
            expect(batch.verdicts(column)).to eq(single[column]), label
          end
        end
        batch.rows.times do |row|
          expect(seen).to be < rows.size, "#{label}: more rows than the corpus lists"
          expect(rows[seen].size).to eq(plan.size), "#{label}: a corpus row of another width"
          plan.each_index do |column|
            assert_cell("#{label}, row #{seen}, column #{column}", batch, column, row, rows[seen][column], notes: notes)
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
    expect(problems).to eq(problems_of(vector, notes: notes)), "#{label}: problems"
    assert_failure(label, failure, vector)
  end

  it "reads every key the corpus has, and notices one added anywhere" do
    expect(CorpusHelpers.keys_problem(CORPUS, workbook: false)).to be_nil
  end

  it "has the cases, and through every door" do
    expect(CORPUS.size).to eq(137)
    expect(CORPUS.count { |vector| vector.key?("suggest") }).to eq(19)
    doors = CORPUS.flat_map { |vector| vector["plan"].map { |entry| entry["door"].to_sym } }.uniq
    expect(doors).to match_array(HyperTabular::DOORS.keys)
  end

  CORPUS.each do |vector|
    name = vector["name"]

    it "replays \"#{name}\" every way it can be cut" do
      input = input_of(vector)
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

      BATCH_ROWS.each do |batch_rows|
        replay("#{name} (notes off, #{batch_rows} rows a batch)", vector, batch_rows, notes: false) do |dialect, plan|
          HyperTabular::DelimitedReader.new(input, dialect, plan, batch_rows: batch_rows, notes: false)
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

  # A batch's faults, asked after every later read, are those it had when it was read.
  CORPUS.each do |vector|
    it "answers \"#{vector['name']}\"'s faults for each batch, held past the next read" do
      plan = vector["plan"].map { |entry| column_of(entry) }
      # Each batch's list asked as it's read, or as a batch held when the read is over.
      read = lambda do |asked|
        said = []
        begin
          source = HyperTabular::DelimitedReader.new(input_of(vector), dialect_of(vector["dialect"]), plan,
                                                     batch_rows: 2)
          while (batch = source.read)
            said << (asked ? batch.faults : batch)
          end
        rescue HyperTabular::TabularError
          nil
        end
        said
      end
      at_once = read.call(true)
      expect(read.call(false).map(&:faults)).to eq(at_once)
      expect(at_once.flatten.size).to eq(vector["problems"].size)
    end
  end

  it "says what to suggest" do
    expect(CORPUS.select { |vector| vector.key?("suggest") }).not_to be_empty
  end

  CORPUS.select { |vector| vector.key?("suggest") }.each do |vector|
    it "suggests what the corpus says of \"#{vector['name']}\", from memory and from a file" do
      name = vector["name"]
      expected = vector["suggest"]
      input = input_of(vector)
      dialect = dialect_of(vector["dialect"])
      declared = expected.key?("format") ? format_of(expected) : nil
      asked = HyperTabular::SuggestOptions.new(sample_rows: expected.fetch("sample_rows", 1000), format: declared)
      found = HyperTabular::DelimitedReader.suggest_string(input, dialect, asked)
      columns = expected["columns"]
      expect(suggestion_json(found)).to eq(reported(columns)), name
      expect(found.truncated).to eq(expected["truncated"]), name
      assert_notation(name, found, columns, declared)
      Tempfile.create(["suggest", ".csv"]) do |file|
        file.binmode
        file.write(input)
        file.close
        expect(HyperTabular::DelimitedReader.suggest(file.path, dialect, asked)).to eq(found), name
      end

      # The plan reads back refusing nothing it resolved, among the rows it sampled.
      sample = expected.fetch("sample_rows", vector["rows"].size)
      [input, ReadOnly.new(input)].each do |source|
        read = 0
        reader = HyperTabular::DelimitedReader.new(source, dialect, found.plan)
        begin
          while (batch = reader.read)
            refused = batch.faults.select do |problem|
              report = problem.plan_column && found.report[problem.plan_column]
              report && report.status == :resolved && report.door != :text && problem.row + read < sample
            end
            expect(refused).to eq([]), name
            read += batch.rows
          end
        rescue HyperTabular::TabularError
          expect(read).to be >= sample, name
        end
        reader.close
      end
    end
  end

  it "fails to suggest only where a read of the case fails" do
    planned = failed = 0
    CORPUS.each do |vector|
      HyperTabular::DelimitedReader.suggest_string(input_of(vector), dialect_of(vector["dialect"]))
      planned += 1
    rescue HyperTabular::TabularError
      failed += 1
      expect(vector).to have_key("failure"), vector["name"]
    end
    expect([planned > 80, failed.positive?]).to eq([true, true])
  end
end
