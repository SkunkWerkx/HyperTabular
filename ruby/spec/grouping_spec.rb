# A column's grouping: Western unless the caller declares Indian on that column, in the same read
# as a column that doesn't, and a stale policy word refused where it's handed over, before any
# cell. What the shapes read is HyperCast's and the corpus's. No test here names a locale: the
# Indian shape is declared, never looked up.
#
# HyperCast's constructor refuses a stale word, so two things stand in for what an old program's
# format would hand over. A 1.0 Marshal payload, made by dumping NumFormaz (a name of NumFormat's
# length, since a Marshal string's length prefix isn't rewritten) and loading it as NumFormat, is the
# one route a real program has: it is refused at HyperTabular's door, Column.specs, which every
# bind, open and suggest goes through, by HyperCast's NumFormat#raw_flags as the format is packed.
# A subclass that packs the word is what gets past that to Runtime.refused, the core's own judge,
# the same door's second half.

require "tempfile"
require "stringio"
require "spec_helper"
require_relative "corpus_helpers"

module HyperCast
  # What 1.0's NumFormat was, under a name to be rewritten in its Marshal payload.
  NumFormaz = Data.define(*NumFormat.members)
end

RSpec.describe "grouping" do
  column = HyperTabular::Column
  reader = HyperTabular::DelimitedReader
  csv = HyperTabular::Dialect::CSV
  indian = HyperCast::NumFormat::INVARIANT.with(grouping: :indian)
  both = %(indian,western\n"1,23,456","1,23,456"\n"1,234,567","1,234,567"\n)
  packages = %w[xlsx ods numbers].map { |kind| File.join(CorpusHelpers.directory, "workbook", "grouping.#{kind}") }
  broken = File.join(CorpusHelpers.directory, "workbook", "broken.xlsx")
  options = HyperTabular::SheetOptions::DEFAULT
  flags_at = (0...HyperCast::NumFormat::PACKED_BYTES).find do |at|
    HyperCast::NumFormat::INVARIANT.packed.getbyte(at) != indian.packed.getbyte(at)
  end & ~3

  # A format whose packed word is +word+.
  packing = lambda do |word|
    packed = HyperCast::NumFormat::INVARIANT.packed.dup
    packed[flags_at, 4] = [word].pack("L<")
    Class.new(HyperCast::NumFormat) { define_method(:packed) { packed } }
         .new(decimal_sep: ".", group_sep: ",", flags: HyperCast::ALL_STYLES)
  end

  # What a program stored under 1.0's HyperCast comes back as: a NumFormat with this flags word.
  marshalled = lambda do |flags|
    members = HyperCast::NumFormat::INVARIANT.to_h.except(:grouping).merge(flags: flags)
    Marshal.load(Marshal.dump(HyperCast::NumFormaz.new(**members)).sub("NumFormaz", "NumFormat"))
  end

  # The word: a real format where HyperCast makes one, a stand-in for what it refuses.
  word_of = lambda do |word|
    { 95 => HyperCast::NumFormat::INVARIANT, 607 => indian }.fetch(word) { packing.call(word) }
  end
  words = { 351 => true, 863 => true, 95 => false, 607 => false }.freeze
  stale_routes = {
    "a Marshal payload with 351" => -> { marshalled.call(351) },
    "a Marshal payload with 863" => -> { marshalled.call(863) },
    "a format packing 351" => -> { packing.call(351) },
    "a format packing 863" => -> { packing.call(863) }
  }.freeze

  planned = ->(format) { [column.i32(0), column.i32(1, format)] }

  def batch_of(source, dialect, plan, **rest)
    HyperTabular::DelimitedReader.new(source, dialect, plan, **rest).read
  end

  # Holds a batch to Indian in one column and Western in the other.
  def holds_both(batch, indian, western)
    expect(batch.get(indian, 0)).to eq(HyperCast::Success.new(value: 123_456))
    expect(batch.get(western, 0).reason).to eq(:malformed)
    expect(batch.get(western, 0).detail).to eq(HyperCast::Detail::GROUP_SIZE)
    expect(batch.get(indian, 1).reason).to eq(:malformed)
    expect(batch.get(western, 1)).to eq(HyperCast::Success.new(value: 1_234_567))
  end

  def rejects(refused, what = /plan column 1 cannot be honoured/, &call)
    if refused
      expect(&call).to raise_error(ArgumentError, what)
    else
      expect(&call).not_to raise_error
    end
  end

  it "stands in for exactly what the real formats pack" do
    expect(HyperCast::NumFormat::INVARIANT.packed[flags_at, 4].unpack1("L<")).to eq(95)
    expect(indian.packed[flags_at, 4].unpack1("L<")).to eq(607)
    expect(packing.call(95).packed).to eq(HyperCast::NumFormat::INVARIANT.packed)
    expect(packing.call(607).packed).to eq(indian.packed)
    expect(batch_of(both, csv, [column.i32(0, packing.call(607))]).get(0, 0))
      .to eq(HyperCast::Success.new(value: 123_456))
    # The payload's route is sound where its word is: 95 reads, Western.
    loaded = marshalled.call(95)
    expect(loaded).to be_a(HyperCast::NumFormat)
    expect(loaded.packed).to eq(HyperCast::NumFormat::INVARIANT.packed)
    expect(batch_of(both, csv, [column.i32(1, loaded)]).get(0, 1)).to eq(HyperCast::Success.new(value: 1_234_567))
  end

  [351, 863].each do |word|
    it "has no way to make #{word} from HyperCast's constructor" do
      expect { HyperCast::NumFormat.new(decimal_sep: ".", group_sep: ",", flags: word & 0xFF, grouping: word & ~0xFF) }
        .to raise_error(ArgumentError)
      expect { HyperCast::NumFormat.new(decimal_sep: ".", group_sep: ",", flags: word) }.to raise_error(ArgumentError)
    end
  end

  it "reads an Indian column beside a Western one" do
    holds_both(batch_of(both, csv, [column.i32(0, indian), column.i32(1)]), 0, 1)
    holds_both(batch_of(StringIO.new(both), csv, [column.i32(0, indian), column.i32(1)]), 0, 1)
    expect(column.i32(1).format.packed[flags_at, 4].unpack1("L<")).to eq(95)
  end

  packages.each do |path|
    it "reads an Indian sheet column beside a Western one in #{File.basename(path)}" do
      batch = HyperTabular::Workbook.open(path).sheet(0, options, [column.i32(2, indian), column.i32(2)]).read
      holds_both(batch, 0, 1)
      # A typed number has no notation: it's the same under either.
      expect(batch.get(0, 2)).to eq(HyperCast::Success.new(value: 1234))
      expect(batch.get(1, 2)).to eq(HyperCast::Success.new(value: 1234))
    end
  end

  stale_routes.each do |route, stale|
    it "refuses #{route} at every delimited entry point, before any cell" do
      plan = planned.call(stale.call)
      Tempfile.create(["grouping", ".csv"]) do |file|
        file.write(both)
        file.close
        rejects(true) { reader.new(both, csv, plan) }
        rejects(true) { reader.new(StringIO.new(both), csv, plan) }
        rejects(true) { reader.open(file.path, csv, plan) }
      end
      unbound = reader.new(both, csv)
      records = unbound.records
      rejects(true) { unbound.bind(plan) }
      expect(unbound).not_to be_bound
      expect(unbound.records).to eq(records)
      expect { unbound.read }.to raise_error(RuntimeError, /no plan/)
    end

    it "refuses #{route} at every sheet entry point" do
      plan = planned.call(stale.call)
      book = HyperTabular::Workbook.open(packages.first)
      rejects(true) { book.sheet(0, options, plan) }
      unbound = book.sheet(0, options)
      rejects(true) { unbound.bind(plan) }
      expect(unbound).not_to be_bound
      expect { unbound.read }.to raise_error(RuntimeError, /no plan/)
    end

    it "refuses #{route} when asked what to declare, first of anything" do
      format = stale.call
      every = HyperTabular::SuggestOptions.new(format: format)
      past = HyperTabular::SuggestOptions.new(column_formats: [[99, format]])
      replaced = HyperTabular::SuggestOptions.new(format: format, column_formats: [[0, indian]])
      twice = HyperTabular::SuggestOptions.new(column_formats: [[0, format], [0, indian]])
      headerless = HyperTabular::Dialect.new(separator: ",", has_header: false)
      cases = [["a\n1\n", csv, every, 0], ["", csv, every, 0], ["", headerless, every, 0],
               ["a\n1\n", csv, past, 99], ["a\n1\n", csv, replaced, 0], ["a\n1\n", csv, twice, 0],
               [%(a\n"1\n), csv, every, 0], [%("1\n), headerless, every, 0]]
      cases.each do |text, dialect, asked, ordinal|
        rejects(true, /plan column #{ordinal} cannot be honoured/) { reader.suggest_string(text, dialect, asked) }
      end
      rejects(true, /plan column 0 cannot be honoured/) { reader.suggest("/no/such/file.csv", csv, every) }
      sheet = HyperTabular::Workbook.open(packages.first).sheet(0, options)
      [every, past, replaced, twice].each { |asked| rejects(true, /cannot be honoured/) { sheet.suggest(asked) } }
      sheet = HyperTabular::Workbook.open(broken).sheet(0, options)
      rejects(true, /cannot be honoured/) { sheet.suggest(every) }
    end
  end

  # The controls: each reaches what it was asked for, or the source's own failure.
  [95, 607].each do |word|
    it "takes #{word} everywhere a stale word is refused, each reaching the source" do
      format = word_of.call(word)
      plan = planned.call(format)
      rejects(false) { reader.new(both, csv, plan) }
      rejects(false) { reader.new(StringIO.new(both), csv).bind(plan) }
      book = HyperTabular::Workbook.open(packages.first)
      rejects(false) { book.sheet(0, options, plan) }
      rejects(false) { book.sheet(0, options).bind(plan) }

      every = HyperTabular::SuggestOptions.new(format: format)
      headerless = HyperTabular::Dialect.new(separator: ",", has_header: false)
      rejects(false) { reader.suggest_string("a\n1\n", csv, every) }
      rejects(false) { reader.suggest_string("", csv, every) }
      rejects(false) { reader.suggest_string("", headerless, every) }
      expect { reader.suggest_string(%(a\n"1\n), csv, every) }.to raise_error(HyperTabular::TabularError)
      expect { reader.suggest_string(%("1\n), headerless, every) }.to raise_error(HyperTabular::TabularError)
      expect { reader.suggest("/no/such/file.csv", csv, every) }.to raise_error(Errno::ENOENT)
      rejects(false) { book.sheet(0, options).suggest(every) }
      sheet = HyperTabular::Workbook.open(broken).sheet(0, options)
      expect { sheet.suggest(every) }.to raise_error(HyperTabular::TabularError)
    end
  end

  it "says which format per column it refuses, and takes a format ahead of Indian on the same column" do
    stale = packing.call(351)
    expect { reader.suggest_string("a\n1\n", csv, HyperTabular::SuggestOptions.new(column_formats: [[99, stale]])) }
      .to raise_error(ArgumentError, /plan column 99 cannot be honoured/)
    twice = HyperTabular::SuggestOptions.new(format: HyperCast::NumFormat::INVARIANT, column_formats: [[0, indian]])
    expect(reader.suggest_string("a\n7\n", csv, twice).report[0].format).to equal(indian)
  end

  it "asks the core itself: Runtime.refused names the first it refuses, 95 and 607 go through" do
    specs = ->(*words_) { words_.map { |word| [column.i32(0, packing.call(word)).packed_with, nil, nil] } }
    expect(HyperTabular::Runtime.refused(specs.call(95, 607))).to be_nil
    expect(HyperTabular::Runtime.refused(specs.call(351))).to eq(0)
    expect(HyperTabular::Runtime.refused(specs.call(95, 863))).to eq(1)
  end

  it "has the core refuse at the first fill what got past the door" do
    allow(HyperTabular::Runtime).to receive(:refused).and_return(nil)
    expect { batch_of(both, csv, [column.i32(0, packing.call(351))]) }.to raise_error(/contract violation|cannot be/)
  end

  it "gives an Indian shape no culture until one is declared" do
    ask = ->(text, asked = HyperTabular::SuggestOptions::DEFAULT) { reader.suggest_string(text, csv, asked) }
    found = ask.call(%(a\n"1,23,456"\n))
    expect([found.report[0].door, found.report[0].status]).to eq(%i[text resolved])
    found = ask.call(%(a\n"1,23,456"\n5\n))
    expect([found.report[0].door, found.report[0].status]).to eq(%i[text unresolved])
    # A suggested plan is Western: flags 95, and a later Indian shape is refused.
    found = ask.call(%(a\n"1,234,567"\n))
    expect(found.report[0].format).to equal(HyperCast::NumFormat::INVARIANT)
    expect(found.plan[0].format.packed[flags_at, 4].unpack1("L<")).to eq(95)
    expect(batch_of(%(a\n"1,23,456"\n), csv, found.plan).get(0, 0).reason).to eq(:malformed)

    # Declared, the column is a number, its plan reads it, and a Western shape is what's refused.
    declared = HyperTabular::SuggestOptions.new(column_formats: [[0, indian]])
    text = %(a,b\n"1,23,456","1,23,456"\n)
    found = ask.call(text, declared)
    expect([found.report[0].door, found.report[0].status]).to eq(%i[i32 resolved])
    expect(found.report[0].format).to equal(indian)
    expect(found.plan[0].format.packed[flags_at, 4].unpack1("L<")).to eq(607)
    expect(batch_of(text, csv, found.plan).get(0, 0)).to eq(HyperCast::Success.new(value: 123_456))
    # The other column asked nothing declared, and stays text.
    expect(found.report[1].door).to eq(:text)
    found = ask.call(%(a\n"1,234,567"\n), declared)
    expect([found.report[0].door, found.report[0].status, found.report[0].format]).to eq([:text, :resolved, nil])
  end

  it "is Western by default, however that is reached" do
    text = %(a\n"1,234,567"\n"1,23,456"\n)
    [column.i32(0), column.new(ordinal: 0, door: :i32), column.i32(0, HyperCast::NumFormat::INVARIANT),
     column.i32(0, marshalled.call(95))].each do |plain|
      batch = batch_of(text, csv, [plain])
      expect(batch.get(0, 0)).to eq(HyperCast::Success.new(value: 1_234_567))
      expect(batch.get(0, 1).reason).to eq(:malformed)
    end
  end

  it "has neither policy read what neither writes" do
    [HyperCast::NumFormat::INVARIANT, indian].each do |format|
      batch = batch_of(%(a\n"12,34"\n"1,2,345"\n), csv, [column.i32(0, format)])
      expect(batch.values(0)).to eq([nil, nil])
      expect(batch.get(0, 0).reason).to eq(:malformed)
    end
  end

  # The 1.0 exports, which no read takes, stay what they were: on the Fiddle path, the one with
  # them in its table, every format is Western there, whatever its word.
  describe "the 1.0 exports" do
    before(:all) do
      skip "the 1.0 exports are reached through Fiddle (BACKEND=#{HyperTabular::BACKEND})" unless
        HyperTabular::BACKEND == :fiddle
    end

    let(:runtime) { HyperTabular::Runtime }

    # A column's arrays and the memory of a 1.0 fill.
    def memory(rows)
      values = runtime.buffer(4 * rows)
      verdicts = runtime.buffer(12 * rows)
      columns = runtime.buffer(16)
      columns[0, 16] = [values.to_i, verdicts.to_i].pack("Q<2")
      window = runtime.buffer(1 << 20)
      arena = runtime.buffer(1 << 20)
      cells = runtime.buffer(8 << 12)
      row = runtime.buffer(16 << 12)
      [values, verdicts, columns, [window, arena, cells, row]]
    end

    def buffers(held, tables = [0] * 6)
      block = runtime.buffer(112)
      block[0, 112] = [held[0].to_i, 1 << 20, held[1].to_i, 1 << 20, held[2].to_i, 1 << 12, held[3].to_i, 1 << 12,
                       *tables].pack("Q<14")
      block
    end

    def pinned(bytes)
      held = runtime.buffer(bytes.bytesize)
      held[0, bytes.bytesize] = bytes
      held
    end

    {
      "351" => -> { packing.call(351) }, "607" => -> { indian }, "863" => -> { packing.call(863) },
      "95" => -> { HyperCast::NumFormat::INVARIANT }
    }.each do |word, format|
      it "reads every format Western through hypertabular_delimited_fill: #{word}" do
        state = runtime.buffer(runtime.function(:hypertabular_delimited_state_size).call)
        expect(runtime.function(:hypertabular_delimited_init).call(state, pinned(csv.packed))).to eq(0)
        text = %("1,234,567"\n"1,23,456"\n)
        values, verdicts, columns, held = memory(2)
        out = runtime.buffer(64)
        code = runtime.function(:hypertabular_delimited_fill).call(state, pinned(text), text.bytesize, 1,
                                                                   pinned(column.i32(0, format.call).packed), columns,
                                                                   1, 2, buffers(held), out)
        expect(code).to eq(0)
        expect(out[0, 8].unpack1("Q<")).to eq(2)
        expect(values[0, 4].unpack1("l<")).to eq(1_234_567)
        expect(verdicts[0, 24].unpack("L<6").values_at(2, 5)).to eq([0, 2])
      end

      it "reads every format Western through hypertabular_workbook_fill: #{word}" do
        book = HyperTabular::Runtime::Book::Opened.new(File.binread(packages.first))
        state = book.copy_of_state
        _name, _hidden, part, index = book.sheets.first
        out = runtime.buffer(64)
        part_held = pinned(part)
        call = runtime.function(:hypertabular_workbook_sheet)
        expect(call.call(state, book.container, book.length, part_held, part.bytesize, index, 1, 1, out)).to eq(0)
        values, verdicts, columns, held = memory(8)
        tables = book.tables.to_a
        expect(runtime.function(:hypertabular_workbook_header).call(state, book.container, book.length,
                                                                    buffers(held, tables), out)).to eq(0)
        code = runtime.function(:hypertabular_workbook_fill).call(state, book.container, book.length,
                                                                  pinned(column.i32(2, format.call).packed), columns,
                                                                  1, 8, buffers(held, tables), out)
        expect(code).to eq(0)
        rows = out[0, 8].unpack1("Q<")
        expect(rows).to be >= 2
        reasons = verdicts[0, 12 * rows].unpack("L<*").each_slice(3).map(&:last)
        # The sheet's first row is 1,23,456, refused, and its second 1,234,567, read.
        expect(reasons.first(2)).to eq([2, 0])
        expect(values[4, 4].unpack1("l<")).to eq(1_234_567)
      end
    end
  end
end
