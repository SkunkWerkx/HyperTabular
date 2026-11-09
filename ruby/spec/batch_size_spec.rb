require "etc"
require "open3"
require "stringio"
require "spec_helper"

# A batch size is the caller's, and the core judges it before anything is allocated.
#
# These can't hurt a host however wrong the code goes. The refusals sit just over the core's limit,
# so if the check went, the arrays asked for are a little over 4 GiB, zeroed and untouched: proved in
# a 4 GiB container with the check reverted. The plan of no columns at 1 << 62 has no arrays. The one
# test that does allocate holds about 125 MiB untouched, measured with the memory written to: a read
# that wrote it all still fits any machine.
RSpec.describe "a batch of any size" do
  huge = 1 << 62
  over_limit = /\Abatch_rows asks for \d+ bytes, past the limit of 4294967296\z/

  let(:book) { HyperTabular::Workbook.new(File.binread(File.expand_path("../../corpus/workbook/basic-1904.xlsx", __dir__))) }

  # /proc where there is one: Alpine's ps is busybox's, which has no -p. Nil where nothing answers.
  def resident_mib
    statm = "/proc/self/statm"
    return File.read(statm).split[1].to_i * Etc.sysconf(Etc::SC_PAGESIZE) / (1 << 20) if File.exist?(statm)

    resident = `ps -o rss= -p #{Process.pid}`.to_i / 1024
    resident if resident.positive?
  rescue SystemCallError
    nil
  end

  it "refuses a batch past the core's limit, by name, before allocating" do
    just_over = /\Abatch_rows asks for 5377130496 bytes, past the limit of 4294967296\z/
    plan = [HyperTabular::Column.i32(0)]
    expect { HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV, plan, batch_rows: 1 << 28) }
      .to raise_error(ArgumentError, just_over)
    expect { HyperTabular::DelimitedReader.new(StringIO.new("a\n1\n"), HyperTabular::Dialect::CSV, plan, batch_rows: 1 << 28) }
      .to raise_error(ArgumentError, just_over)
    options = HyperTabular::SheetOptions.new(batch_rows: 1 << 27)
    expect { book.sheet(0, options, plan) }
      .to raise_error(ArgumentError, /\Abatch_rows asks for 4831838224 bytes, past the limit of 4294967296\z/)
  end

  it "refuses a late plan, and leaves the reader and the sheet unbound" do
    plan = [HyperTabular::Column.i32(0)]
    reader = HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV, batch_rows: 1 << 28)
    expect { reader.bind(plan) }.to raise_error(ArgumentError, over_limit)
    expect(reader).not_to be_bound
    sheet = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 1 << 27))
    expect { sheet.bind(plan) }.to raise_error(ArgumentError, over_limit)
    expect(sheet).not_to be_bound
  end

  it "reads a plan of no columns at any batch size, delimited: it has no arrays" do
    reader = HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV, [], batch_rows: huge)
    expect(reader.read.rows).to eq(1)
    expect(reader.read).to be_nil
  end

  it "refuses a plan of no columns where a row's width has to be kept, or a sheet's cell table" do
    reporting = HyperTabular::Dialect.new(separator: ",", long_rows: :report)
    expect { HyperTabular::DelimitedReader.new("a\n1\n", reporting, [], batch_rows: 1 << 31) }
      .to raise_error(ArgumentError, over_limit)
    expect { book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 1 << 30), []) }
      .to raise_error(ArgumentError, over_limit)
  end

  it "refuses a zero, for each option, where an omitted size has its default" do
    plan = [HyperTabular::Column.i32(0)]
    text = "a\n1\n"
    no_rows = /\Abatch_rows must be at least 1\z/
    no_buffer = /\Abuffer_bytes must be at least 1\z/
    expect { HyperTabular::DelimitedReader.new(text, HyperTabular::Dialect::CSV, plan, batch_rows: 0) }
      .to raise_error(ArgumentError, no_rows)
    expect { HyperTabular::DelimitedReader.new(text, HyperTabular::Dialect::CSV, batch_rows: 0) }
      .to raise_error(ArgumentError, no_rows)
    expect { HyperTabular::DelimitedReader.new(StringIO.new(text), HyperTabular::Dialect::CSV, plan, buffer_bytes: 0) }
      .to raise_error(ArgumentError, no_buffer)
    none = HyperTabular::SheetOptions.new(batch_rows: 0)
    expect { book.sheet(0, none, plan) }.to raise_error(ArgumentError, no_rows)
    expect { book.sheet(0, none) }.to raise_error(ArgumentError, no_rows)

    expect(HyperTabular::SheetOptions.new.batch_rows).to eq(HyperTabular::DelimitedReader::DEFAULT_BATCH_ROWS)
    omitted = HyperTabular::DelimitedReader.new(StringIO.new(text), HyperTabular::Dialect::CSV, plan)
    expect(omitted.batch_rows).to eq(HyperTabular::DelimitedReader::DEFAULT_BATCH_ROWS)
    expect(omitted.read.rows).to eq(1)
    sampled = HyperTabular::DelimitedReader.suggest_string(text, HyperTabular::Dialect::CSV,
                                                           HyperTabular::SuggestOptions.new(sample_rows: 0))
    expect(sampled.report[0].sampled).to eq(1)
  end

  it "judges a path's sizes before the file is touched" do
    plan = [HyperTabular::Column.i32(0)]
    nowhere = "no such file"
    expect { HyperTabular::DelimitedReader.open(nowhere, HyperTabular::Dialect::CSV) }.to raise_error(Errno::ENOENT)
    expect { HyperTabular::DelimitedReader.open(nowhere, HyperTabular::Dialect::CSV, batch_rows: 0) }
      .to raise_error(ArgumentError, /\Abatch_rows must be at least 1\z/)
    expect { HyperTabular::DelimitedReader.open(nowhere, HyperTabular::Dialect::CSV, buffer_bytes: 0) }
      .to raise_error(ArgumentError, /\Abuffer_bytes must be at least 1\z/)
    expect { HyperTabular::DelimitedReader.open(nowhere, HyperTabular::Dialect::CSV, plan, batch_rows: 1 << 28) }
      .to raise_error(ArgumentError, over_limit)
  end

  it "isn't refused a suggestion for the size of its own batch" do
    width = 268_000
    names = (0...width).map { |column| "c#{column}" }.join(",")
    row = (["1"] * width).join(",")
    wide = HyperTabular::DelimitedReader.suggest_string("#{names}\n#{row}\n#{row}\n", HyperTabular::Dialect::CSV)
    expect(wide.report.size).to eq(width)
    expect(wide.report[width - 1].sampled).to eq(2)
    expect(wide.truncated).to be(false)
    # At exactly the limit with no plan, and a sample past every batch.
    large = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 1 << 29))
                .suggest(HyperTabular::SuggestOptions.new(sample_rows: 1 << 40))
    modest = book.sheet(0, HyperTabular::SheetOptions::DEFAULT)
                 .suggest(HyperTabular::SuggestOptions.new(sample_rows: 4096))
    expect(large).to eq(modest)
  end

  it "samples the same in any number of batches, delimited and sheet" do
    text = "id,price,when,note\n1,\"1,234.50\",2024-01-02,plain\n2,1.234,2024-02-03,\n3,,2024-03-04,\"a, b\"\n" \
           "x,0.5,03/04/2024,\"two\nlines\"\n5,12,2024-05-06,more\n6,13,,more\n"
    csv = HyperTabular::Dialect::CSV
    [1, 2, 3, 5, 6, 1000].each do |limit|
      options = HyperTabular::SuggestOptions.new(sample_rows: limit)
      whole = HyperTabular::DelimitedReader.suggest_string(text, csv, options)
      [1, 2, 3, 7].each do |rows|
        reader = HyperTabular::DelimitedReader.new(text, csv, batch_rows: rows, notes: false)
        sample = HyperTabular::Sample.new(reader.header.size, limit)
        reader.bind(Array.new(reader.header.size) { |ordinal| HyperTabular::Column.text(ordinal) })
        sample.read_from(reader)
        expect(sample.judge(reader.header, options)).to eq(whole)
      end
    end
    [1, 2, 3, 1000].each do |limit|
      options = HyperTabular::SuggestOptions.new(sample_rows: limit)
      whole = book.sheet(0, HyperTabular::SheetOptions::DEFAULT).suggest(options)
      [1, 2, 3].each do |rows|
        sheet = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: rows, notes: true))
        sample = HyperTabular::Sample.new(sheet.header.size, limit, typed: true)
        sheet.bind(HyperTabular::Sample.sheet_plan(sheet.header.size))
        sample.read_from(sheet)
        expect(sample.judge(sheet.header, options)).to eq(whole)
      end
    end
    # A control: the sample's size does move the answer, so the comparison above can fail.
    one = HyperTabular::DelimitedReader.suggest_string(text, csv, HyperTabular::SuggestOptions.new(sample_rows: 1))
    six = HyperTabular::DelimitedReader.suggest_string(text, csv, HyperTabular::SuggestOptions.new(sample_rows: 6))
    expect(one).not_to eq(six)
  end

  it "samples the same with no header, ragged rows and row widths kept, across each batch boundary" do
    ragged = "1,x\n2,y,long\n3\n4,z\n5,w\n"
    headerless = HyperTabular::Dialect::CSV.with(has_header: false, short_rows: :pad, long_rows: :report)
    (1..6).each do |limit|
      options = HyperTabular::SuggestOptions.new(sample_rows: limit)
      whole = HyperTabular::DelimitedReader.suggest_string(ragged, headerless, options)
      expect(whole.report[0].sampled).to eq([limit, 5].min)
      expect(whole.truncated).to be(limit < 5)
      (1..4).each do |rows|
        reader = HyperTabular::DelimitedReader.new(ragged, headerless, batch_rows: rows, notes: false)
        sample = HyperTabular::Sample.new(2, limit)
        reader.bind([HyperTabular::Column.text(0), HyperTabular::Column.text(1)])
        sample.read_from(reader)
        expect(sample.judge(reader.header, options)).to eq(whole)
      end
    end
  end

  it "names a row's width where one row alone is past the limit, which no public plan reaches" do
    spec = HyperTabular::Column.specs([HyperTabular::Column.i32(0)])[0]
    far = [spec[0].dup.tap { |packed| packed[4, 4] = [0xFFFF_FFFF].pack("L<") }, spec[1], spec[2]]
    expect { HyperTabular::Runtime.check_sizes([far], 1, 1, HyperTabular::Runtime::BATCH_SHEET) }
      .to raise_error(ArgumentError,
                      "a row of 4294967296 columns asks for 68719476768 bytes, past the limit of 4294967296")
  end

  it "starts a buffer past the longest record where the core says" do
    reader = HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV, buffer_bytes: 1 << 40)
    expect(reader.instance_variable_get(:@buffer_bytes)).to eq(1 << 30)
    expect(HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV, buffer_bytes: 7)
      .instance_variable_get(:@buffer_bytes)).to eq(7)
  end

  it "keeps what a count has to be for the core to be asked" do
    %i[batch_rows buffer_bytes].each do |name|
      [[-1, /negative/], ["1", /Integer/], [nil, /Integer/], [1 << 64, /64 bits/]].each do |count, why|
        expect { HyperTabular::DelimitedReader.new(StringIO.new(""), HyperTabular::Dialect::CSV, name => count) }
          .to raise_error(ArgumentError, /\A#{name} .*#{why.source}/)
      end
    end
    expect { book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: "1")) }.to raise_error(ArgumentError, /Integer/)
  end

  # The bytes a declared encoding has decoded, and where the core stands, both hold until the new
  # buffer is whole, so a refusal along the way leaves a read that can be asked again.
  [["decoded", { encoding: :latin1 }], ["undecoded", {}]].each do |name, dialect|
    it "reads on when an allocation is refused during a refill, #{name}" do
      text = "a\n#{(1..200).to_a.join("\n")}\n"
      dialect = HyperTabular::Dialect.new(separator: ",", **dialect)
      reader = HyperTabular::DelimitedReader.new(StringIO.new(text), dialect, [HyperTabular::Column.i32(0)],
                                                 batch_rows: 7, buffer_bytes: 16)
      armed = true
      original = String.instance_method(:<<)
      failing = Module.new do
        define_method(:<<) do |other|
          if armed && other.is_a?(String) && other.encoding == Encoding::UTF_8 && other.match?(/\A\d/n)
            armed = false
            raise NoMemoryError, "no"
          end
          original.bind_call(self, other)
        end
      end
      found = []
      reader.read.then { |batch| found.concat(batch.values(0)) } # the header's refill has come and gone
      String.prepend(failing)
      begin
        begin
          found.concat(reader.read.values(0)) while armed
        rescue NoMemoryError
          nil
        end
        expect(armed).to be(false)
      ensure
        armed = false
      end
      while (batch = reader.read)
        found.concat(batch.values(0))
      end
      expect(found).to eq((1..200).to_a)
    end
  end

  it "doesn't write to the memory a large batch asks for, and reads through it" do
    before = resident_mib
    skip "nothing here says what a process holds in memory" if before.nil?

    rows = 1 << 21
    reader = HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV, [HyperTabular::Column.i32(0)],
                                               batch_rows: rows)
    expect(reader.read.values(0)).to eq([1])
    sheet = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: rows), [HyperTabular::Column.text(0)])
    expect(sheet.read.rows).to be_positive
    expect(resident_mib - before).to be < 32
    expect([reader, sheet]).to all(respond_to(:read)) # both held to here
  end

  it "reads a sheet the same at a large batch as a row at a time" do
    plan = [HyperTabular::Column.text(0)]
    default = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 1), plan)
    large = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 1 << 12), plan)
    expect(large.read.values(0)).to eq(default.each.map { |row| row.value(0) })
  end

  context "on the Fiddle backend" do
    before { skip "the Magnus extension doesn't allocate here" unless HyperTabular::BACKEND == :fiddle }

    it "answers zeroed memory, of the size asked, freed by what allocated it" do
      [0, 1, 64, 1 << 20].each do |size|
        block = HyperTabular::Runtime.buffer(size)
        expect(block.size).to eq(size)
        expect(block.free.to_i).to eq(Fiddle::RUBY_FREE)
        expect(block[0, size]).to eq("\0".b * size) if size.positive?
        expect(block.null?).to be(false)
      end
    end

    it "refuses a size past an address space before asking" do
      expect { HyperTabular::Runtime.buffer(1 << 63) }.to raise_error(NoMemoryError)
      expect { HyperTabular::Runtime.buffer(1 << 80) }.to raise_error(NoMemoryError)
    end

    # 64-bit Windows, as far as a Mac or Linux can stand in for it: its long is 32 bits, and
    # Fiddle::Pointer.new would raise on the size after the allocation had been made.
    it "refuses a size a Fiddle::Pointer can't take before asking for it" do
      stub_const("Fiddle::SIZEOF_LONG", 4)
      expect(HyperTabular::Runtime.send(:zeroing)).not_to receive(:call)
      expect { HyperTabular::Runtime.buffer(1 << 31) }.to raise_error(NoMemoryError)
    end

    # calloc, as TruffleRuby and JRuby's Fiddle get it, answers null where ruby_xcalloc raises.
    it "raises where the allocator answers null, rather than handing out a null pointer" do
      HyperTabular::Runtime.buffer(1) # the library is loaded
      allow(HyperTabular::Runtime).to receive(:zeroing).and_return(double(call: Fiddle::Pointer.new(0)))
      expect { HyperTabular::Runtime.buffer(64) }.to raise_error(NoMemoryError, "failed to allocate memory")
    end

    it "says no at the question, when the allocator isn't in this Ruby" do
      script = <<~RUBY
        require "fiddle"
        Fiddle::Handle.prepend(Module.new do
          define_method(:[]) { |name| name == "ruby_xcalloc" ? raise(Fiddle::DLError, "unknown symbol") : super(name) }
        end)
        require "hypertabular"
        print HyperTabular.available?, " "
        begin
          HyperTabular::DelimitedReader.new("a\\n1\\n", HyperTabular::Dialect::CSV, [HyperTabular::Column.i32(0)])
        rescue LoadError => e
          print e.message
        end
      RUBY
      out, status = Open3.capture2e(RbConfig.ruby, "-I", File.expand_path("../lib", __dir__), "-e", script)
      expect(status.success?).to be(true), out
      expect(out).to start_with("false ").and include("ruby_xcalloc")
    end

    # Each read below asks for a larger block once, and is told no: the block and what the core is
    # told of it must still agree, or the next call hands it a capacity it doesn't have.
    def told_no_once(kernel)
      held = %i[@arena_cap @names_cap @cells_cap @per_row].to_h { |name| [name, kernel.instance_variable_get(name)] }
      armed = true
      allow(HyperTabular::Runtime).to receive(:buffer).and_wrap_original do |original, size|
        if armed
          armed = false
          raise NoMemoryError, "no"
        end
        original.call(size)
      end
      expect { yield }.to raise_error(NoMemoryError)
      expect(armed).to be(false)
      expect(held.to_h { |name, _| [name, kernel.instance_variable_get(name)] }).to eq(held)
    end

    it "keeps the arena it has when a larger one is refused, and reads on" do
      long = "x\"\"" * 5000
      reader = HyperTabular::DelimitedReader.new("a\n\"#{long}\"\n", HyperTabular::Dialect::CSV, [HyperTabular::Column.text(0)])
      told_no_once(reader.instance_variable_get(:@kernel)) { reader.read }
      expect(reader.read.values(0)).to eq(["x\"" * 5000])
    end

    it "keeps the cell table it has when a larger one is refused, and reads on" do
      kernel = HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV, [HyperTabular::Column.text(0)])
                                            .instance_variable_get(:@kernel)
      told_no_once(kernel) { kernel.send(:grow_cells, 300, 4096) }
      kernel.send(:grow_cells, 300, 4096)
      rows = HyperTabular::Runtime::Delimited.cell_rows(300, 4096)
      expect([kernel.per_row, kernel.cell_entries]).to eq([300, 300 * rows])
    end

    it "keeps the names it has when a larger table is refused, and reads on" do
      names = (0...300).map { |column| "c#{column}" }.join(",")
      reader = HyperTabular::DelimitedReader.new("a\n1\n", HyperTabular::Dialect::CSV.with(has_header: false), nil)
      kernel = reader.instance_variable_get(:@kernel)
      kernel.instance_variable_set(:@names_cap, 64) # as #header starts, ahead of the refusal under test
      span = HyperTabular::Runtime::Delimited::SPAN_BYTES
      kernel.instance_variable_set(:@names, HyperTabular::Runtime.buffer(span * 64))
      text = "#{names}\n"
      kernel.attach(text)
      told_no_once(kernel) { kernel.header(0, text.bytesize, true) }
      expect(kernel.header(0, text.bytesize, true)).to eq(HyperTabular::Runtime::Delimited::OK)
    end

    it "refuses by the allocator a size within an address space no process has" do
      expect { HyperTabular::Runtime.buffer(1 << 62) }.to raise_error(NoMemoryError)
    end
  end
end
