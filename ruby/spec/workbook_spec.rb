# Replays corpus/workbook.json — the contract every binding replays, and the one the Rust
# binding replays — through this binding: each package opened from a String, from its path
# and from an IO, each sheet read by index and (where the name finds it) by name, in batches
# of one row, of two, and of more than any sheet has, with its plan up front and header
# first. Each batch's faults list is held to the case's problems, a case's suggest to what its sheet
# says, and a key this replay doesn't know fails it. And the workbook's own surface around that.

require "spec_helper"
require_relative "corpus_helpers"

RSpec.describe HyperTabular::Workbook do
  include CorpusHelpers

  CASES = CorpusHelpers.corpus("workbook.json")
  WORKBOOK_BATCH_ROWS = [1, 2, 1024].freeze

  # An IO with nothing but #read, handing over a few bytes at a time however much is asked
  # for — the shape of a pipe or a socket — and that knows whether it was closed.
  class Trickle
    def initialize(bytes)
      @bytes = bytes
      @at = 0
      @closed = false
    end

    def read(length = nil, buffer = nil)
      raise IOError, "closed stream" if @closed
      return nil if length && @at >= @bytes.bytesize

      taken = @bytes.byteslice(@at, length ? [length, 7].min : @bytes.bytesize - @at)
      @at += taken.bytesize
      length.nil? || buffer.nil? ? taken : buffer.replace(taken)
    end

    def close
      @closed = true
    end

    def closed? = @closed
  end

  def openings(path)
    {
      "string" => -> { described_class.new(File.binread(path)) }, "path" => -> { described_class.open(path) },
      "file IO" => lambda {
        file = File.open(path, "rb")
        described_class.new(file, close_source: true).tap { expect(file).to be_closed }
      },
      "#read-only IO" => -> { described_class.new(Trickle.new(File.binread(path))) }
    }
  end

  # A sheet opened without a plan, held to what it is before its plan comes — unbound, with
  # an empty plan and its header read — and then bound to +plan+.
  def header_first(vector, plan)
    sheet = yield
    expect(sheet).not_to be_bound
    expect(sheet.plan).to eq([])
    expect(sheet.header).to eq(vector["header"])
    sheet.bind(plan)
  end

  def replay_sheet(label, vector, plan, sheet, batch_rows, notes: true)
    rows = vector["rows"]
    numbers = vector["numbers"]
    expect(sheet.header).to eq(vector["header"]), label
    expect(sheet.plan).to eq(plan), label
    seen = 0
    failure = nil
    problems = []
    begin
      while (batch = sheet.read)
        expect(batch.rows).to be_between(1, batch_rows), label
        problems.concat(batch.faults.map { |problem| problem_json(batch, problem, seen) })
        assert_rows(label, plan, batch)
        batch.rows.times do |row|
          expect(seen).to be < rows.size, "#{label}: more rows than the corpus lists"
          expect(rows[seen].size).to eq(plan.size), "#{label}: a corpus row of another width"
          expect(batch.line(row)).to eq(numbers[seen]), "#{label}, row #{seen}"
          plan.each_index do |column|
            assert_cell("#{label}, row #{seen}, column #{column}", batch, column, row, rows[seen][column], notes: notes)
          end
          seen += 1
        end
      end
    rescue HyperTabular::TabularError => e
      failure = e
      # A failed sheet stays failed: the same failure, again.
      expect { sheet.read }.to raise_error(HyperTabular::TabularError) { |again| expect(again).to equal(e) }
    end
    expect(seen).to eq(rows.size), label
    expect(numbers.size).to eq(seen), label
    expect(problems).to eq(problems_of(vector, notes: notes)), "#{label}: problems"
    assert_failure(label, failure, vector)
  end

  it "reads every key the corpus has, and notices one added anywhere" do
    expect(CorpusHelpers.keys_problem(CASES, workbook: true)).to be_nil
  end

  it "has the cases, and the cells it says it does" do
    expect(CASES.size).to eq(404)
    expect(CASES.map { |vector| vector["file"] }.uniq.size).to eq(42)
    expect(CASES.count { |vector| vector.key?("suggest") }).to eq(15)
    expect(CASES.select { |vector| vector.key?("sheet") }.sum { |vector| vector["rows"].size * vector["plan"].size })
      .to be >= 12_000
  end

  CASES.each do |vector|
    name = vector["name"]

    it "replays \"#{name}\" every way it can be opened and read" do
      path = File.join(CorpusHelpers.directory, vector["file"])
      # A package the core refuses: every way of opening it gives the one failure.
      unless vector.key?("sheet")
        openings(path).each do |source, open|
          expect { open.call }.to raise_error(HyperTabular::TabularError) { |e|
            assert_failure("#{name} (#{source})", e, vector)
          }
        end
        next
      end

      plan = vector["plan"].map { |entry| column_of(entry) }
      index = vector["sheet"]
      sheet_name = vector["sheets"][index]["name"]
      settings = vector["options"]
      openings(path).each do |source, open|
        book = open.call
        expect(book.format.to_s).to eq(vector["format"]), name
        expect(book.date_system).to eq(HyperCast::EXCEL_EPOCHS.key(vector["epoch"])), name
        expect(book.sheets.map { |sheet| [sheet.name, sheet.hidden] })
          .to eq(vector["sheets"].map { |sheet| [sheet["name"], sheet["hidden"]] }), name
        by_name = book.sheets.index { |sheet| sheet.name == sheet_name } == index
        WORKBOOK_BATCH_ROWS.each do |batch_rows|
          options = options_of(settings, batch_rows)
          (by_name ? [index, sheet_name] : [index]).each do |which|
            label = "#{name}: #{source}, #{batch_rows} rows a batch, sheet #{which.inspect}"
            replay_sheet(label, vector, plan, book.sheet(which, options, plan), batch_rows)
            sheet = header_first(vector, plan) { book.sheet(which, options) }
            replay_sheet("#{label}, header first", vector, plan, sheet, batch_rows)
          end
        end
      end
    end
  end

  CASES.select { |vector| vector.key?("sheet") }.each do |vector|
    it "reads \"#{vector['name']}\" the same with notes off, less the details" do
      plan = vector["plan"].map { |entry| column_of(entry) }
      book = described_class.open(File.join(CorpusHelpers.directory, vector["file"]))
      WORKBOOK_BATCH_ROWS.each do |batch_rows|
        sheet = book.sheet(vector["sheet"], options_of(vector["options"], batch_rows, notes: false), plan)
        replay_sheet("#{vector['name']}: notes off", vector, plan, sheet, batch_rows, notes: false)
      end
    end

    it "answers \"#{vector['name']}\"'s faults for each batch, held past the next read" do
      plan = vector["plan"].map { |entry| column_of(entry) }
      book = described_class.open(File.join(CorpusHelpers.directory, vector["file"]))
      # Each batch's list asked as it's read, or as a batch held when the read is over.
      read = lambda do |asked|
        said = []
        begin
          source = book.sheet(vector["sheet"], options_of(vector["options"], 2), plan)
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

  CASES.select { |vector| vector.key?("suggest") }.each do |vector|
    it "suggests what the corpus says of \"#{vector['name']}\", and its plan reads back refusing nothing" do
      name = vector["name"]
      expected = vector["suggest"]
      declared = expected.key?("format") ? format_of(expected) : nil
      asked = HyperTabular::SuggestOptions.new(format: declared)
      options = options_of(vector["options"], 4096)
      path = File.join(CorpusHelpers.directory, vector["file"])
      [described_class.open(path), described_class.new(Trickle.new(File.binread(path)))].each do |book|
        found = book.sheet(vector["sheet"], options).suggest(asked)
        columns = expected["columns"]
        expect(suggestion_json(found)).to eq(reported(columns)), name
        expect(found.truncated).to eq(expected["truncated"]), name
        assert_notation(name, found, columns, declared)
        book.sheet(vector["sheet"], options, found.plan).each_batch do |batch|
          refused = batch.faults.select { |problem| %i[malformed out_of_range].include?(problem.reason) }
          expect(refused).to eq([]), name
        end
      end
    end
  end

  # The corpus again with every buffer starting at one element and never asked to be large up
  # front: each read stops wherever the core runs out of room and resumes in a grown buffer
  # that has to have kept what the old one held. A grow that dropped it would read garbage,
  # and the corpus would say so.
  it "reads every case the same when its buffers start with no room" do
    HyperTabular::Runtime::Book.stingy = true
    HyperTabular::Runtime::Book.grown = [0, 0, 0]
    CASES.select { |vector| vector.key?("sheet") }.each do |vector|
      path = File.join(CorpusHelpers.directory, vector["file"])
      plan = vector["plan"].map { |entry| column_of(entry) }
      options = options_of(vector["options"], 2)
      book = described_class.open(path)
      replay_sheet("#{vector['name']} (stingy)", vector, plan, book.sheet(vector["sheet"], options, plan), 2)
      sheet = header_first(vector, plan) { book.sheet(vector["sheet"], options) }
      replay_sheet("#{vector['name']} (stingy, header first)", vector, plan, sheet, 2)
    end
    # Not once per call: many times, mid-part, for each of the three.
    expect(HyperTabular::Runtime::Book.grown).to all(be > 100)
  ensure
    HyperTabular::Runtime::Book.stingy = false
  end

  it "says what is not there in its own way" do
    book = described_class.open(File.join(CorpusHelpers.directory, "workbook", "basic.xlsx"))
    plan = [HyperTabular::Column.text(0)]
    defaults = HyperTabular::SheetOptions::DEFAULT
    expect { book.sheet("No such sheet", defaults, plan) }.to raise_error(KeyError)
    expect { book.sheet(99, defaults, plan) }.to raise_error(IndexError)
    expect { book.sheet(:first, defaults, plan) }.to raise_error(ArgumentError)
    expect { book.sheet(0, {}, plan) }.to raise_error(ArgumentError, /SheetOptions/)
    expect {
      book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 0), plan)
    }.to raise_error(ArgumentError, /batch_rows/)
    expect { described_class.open("/no/such/file.xlsx") }.to raise_error(Errno::ENOENT)
    expect { described_class.new("not a zip at all") }.to raise_error(HyperTabular::TabularError) { |e|
      expect([e.kind, e.message]).to eq([:not_a_zip, "The workbook is not a zip file."])
    }
    expect(book.inspect).to eq("#<HyperTabular::Workbook xlsx sheets=1>")
  end

  it "reads two sheets at once, each with its own buffers, and batches that outlive them" do
    book = described_class.open(File.join(CorpusHelpers.directory, "workbook", "basic.xlsx"))
    plan = [HyperTabular::Column.text(1), HyperTabular::Column.i64(0)]
    first = book.sheet(0, HyperTabular::SheetOptions::DEFAULT, plan)
    second = book.sheet(0, HyperTabular::SheetOptions.new(has_header: false), plan)
    headed = first.read
    bare = second.read
    expect([first.header&.first, second.header]).to eq(["id", nil])
    expect(bare.rows).to eq(headed.rows + 1)
    # Row 5 of the sheet is empty, and skipped: a row number is where the row was.
    expect(headed.rows.times.map { |row| headed.line(row) }).to eq([2, 3, 4, 6])
    expect(first.read).to be_nil
    expect(first.each_batch.to_a).to eq([])
    expect(headed.values(1).first).to eq(1)
  end
end
