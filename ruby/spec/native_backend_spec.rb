require "spec_helper"
require "open3"
require_relative "agreement"

# Cross-backend agreement: the Magnus extension and the pure-Fiddle fallback must be
# indistinguishable through the public surface. The whole suite already runs under both
# backends (HYPERTABULAR_PURE=1 forces Fiddle); this file pins the *agreement* between them
# by comparing deterministic outputs across a subprocess boundary, as hypercast's does.
RSpec.describe "native backend" do
  before(:all) do
    skip "Magnus extension not loaded (BACKEND=#{HyperTabular::BACKEND})" unless
      HyperTabular::BACKEND == :native
  end

  def fiddle_eval(expression)
    lib = File.expand_path("../lib", __dir__)
    out, status = Open3.capture2(
      { "HYPERTABULAR_PURE" => "1" },
      RbConfig.ruby, "-I", lib, "-r", "hypertabular", "-r", File.expand_path("agreement", __dir__),
      "-e", "print (#{expression})"
    )
    raise "fiddle subprocess failed: #{out}" unless status.success?

    out
  end

  it "reports the native backend, and Fiddle when forced" do
    expect(HyperTabular::BACKEND).to eq(:native)
    expect(fiddle_eval("HyperTabular::BACKEND")).to eq("fiddle")
    expect(fiddle_eval("HyperTabular.native_version")).to eq(HyperTabular.native_version)
  end

  it "loads under -w without a method-redefinition warning" do
    # 1.0.0 printed six lines under `ruby -w`, "method redefined; discarding old start" and
    # "previous definition of start was here" for Delimited.start, Delimited.version and
    # Runtime.unescape: the Fiddle definitions sat on the singleton classes, exactly where
    # the extension then defined its own. hypercast loads first with warnings off, so this
    # holds this gem's own require, whatever the installed hypercast says.
    lib = File.expand_path("../lib", __dir__)
    script = '$VERBOSE = nil; require "hypercast"; $VERBOSE = true; require "hypertabular"; ' \
             "print HyperTabular::BACKEND"
    out, status = Open3.capture2e(RbConfig.ruby, "-I", lib, "-e", script)
    expect([status.success?, out]).to eq([true, "native"])
  end

  # Every cell of both corpora, in batches small enough that every read stops and resumes,
  # said as text and hashed: the verdicts, the raw text, the lines, the headers, the
  # failures.
  it "agrees with the Fiddle backend on every read in both corpora" do
    expect(fiddle_eval("Agreement.digest")).to eq(Agreement.digest)
  end

  # The same, header first: every reader and sheet bound after its header has been read —
  # which for a sheet is the read whose header row's slots must outlive the bind — every
  # workbook opened from an IO, and every row walked as a Row.
  it "agrees with the Fiddle backend on every read in both corpora, header first" do
    expect(fiddle_eval("Agreement.header_first_digest")).to eq(Agreement.header_first_digest)
  end

  # Every suggestion, and what a suggest of every other case says.
  it "agrees with the Fiddle backend on what to suggest for every case in both corpora" do
    expect(fiddle_eval("Agreement.suggest_digest")).to eq(Agreement.suggest_digest)
  end

  it "agrees with the Fiddle backend on a sheet whose repeated header row a late plan reads" do
    path = File.expand_path("../../corpus/workbook/generated-a.ods", __dir__).dump
    script = "s = HyperTabular::Workbook.open(#{path}).sheet(1, HyperTabular::SheetOptions::DEFAULT); " \
             "s.bind(s.header.each_index.map { |i| HyperTabular::Column.text(i) }); s.map(&:to_a).inspect"
    expect(fiddle_eval(script)).to eq(eval(script))
  end

  it "agrees with the Fiddle backend on a fault span in multi-byte text" do
    script = 'r = HyperTabular::DelimitedReader.new("n\n\u00e9\u00e9x\n", HyperTabular::Dialect::CSV, ' \
             "[HyperTabular::Column.i32(0)]); b = r.read; [b.get(0, 0), b.raw(0, 0)].inspect"
    expect(fiddle_eval(script)).to eq(eval(script))
  end

  it "hands every batch its own bytes, untouched by the next read" do
    reader = HyperTabular::DelimitedReader.new("n\n1\n2\n", HyperTabular::Dialect::CSV,
                                               [HyperTabular::Column.i32(0)], batch_rows: 1)
    first = reader.read
    second = reader.read
    expect([first.values(0), second.values(0)]).to eq([[1], [2]])
  end

  # The extension's buffers are Rust's, outside Ruby's heap, where the Fiddle backend's are Strings.
  # Unless it says what each object holds, the collector counts a reader as a few words: a process
  # with room in its heap then makes readers by the thousand without ever collecting one (20,000 of
  # them, each long dead, held 3 GB). A subprocess, so the collector can be held off while it counts.
  it "tells the collector what a reader, a workbook and a sheet hold outside its heap" do
    lib = File.expand_path("../lib", __dir__)
    script = <<~RUBY
      require "objspace"
      rows = 65_536
      plan = Array.new(8) { HyperTabular::Column.f64(0) }
      # A value, a verdict and a note a row, in every column: 12 MiB that no read here touches.
      held = plan.size * rows * (8 + 12 + 4)
      GC.start
      GC.disable
      before = GC.stat(:malloc_increase_bytes)
      reader = HyperTabular::DelimitedReader.new("n\n1\n", HyperTabular::Dialect::CSV, plan, batch_rows: rows)
      counted = GC.stat(:malloc_increase_bytes) - before
      sized = ObjectSpace.memsize_of(reader.instance_variable_get(:@kernel))

      book = HyperTabular::Workbook.open(#{File.expand_path('../../corpus/workbook/basic.xlsx', __dir__).dump})
      before = GC.stat(:malloc_increase_bytes)
      sheet = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: rows), plan)
      sheet_counted = GC.stat(:malloc_increase_bytes) - before
      sheet_sized = ObjectSpace.memsize_of(sheet.instance_variable_get(:@reading))
      book_sized = ObjectSpace.memsize_of(book.instance_variable_get(:@book))
      print [counted >= held, sized >= held, sheet_counted >= held, sheet_sized >= held, book_sized > 1024].inspect
    RUBY
    out, status = Open3.capture2e(RbConfig.ruby, "-I", lib, "-r", "hypertabular", "-e", script)
    expect([status.success?, out]).to eq([true, "[true, true, true, true, true]"])
  end

  it "raises the structural failure a broken workbook is, as the Fiddle backend does" do
    script = 'begin; HyperTabular::Workbook.new("PK\x03\x04 not a zip".b); rescue HyperTabular::TabularError => e; ' \
             "[e.kind, e.message]; end.inspect"
    expect(fiddle_eval(script)).to eq(eval(script))
  end

  # The extension caches Ruby objects in Rust statics, out of the garbage collector's sight. A
  # constant keeps them from being collected but not from being moved, so each is pinned when the
  # extension loads; unpinned, a compacting collection moved them and the next call used whatever
  # took their place (a segfault, or `undefined method 'stingy' for an instance of Array` in
  # HyperTabular run 37718484155). Every movable object is moved here first, in a subprocess so a
  # regression fails this example, not the run.
  it "keeps working after a compacting collection has moved everything it can" do
    skip "this Ruby's GC does not compact" unless GC.respond_to?(:verify_compaction_references)

    lib = File.expand_path("../lib", __dir__)
    script = <<~RUBY
      GC.verify_compaction_references(expand_heap: true, toward: :empty)
      book = HyperTabular::Workbook.open(#{File.expand_path("../../corpus/workbook/basic-1904.xlsx", __dir__).dump})
      print HyperTabular::BACKEND, " ", book.sheets.map(&:name).inspect
      begin
        HyperTabular::Workbook.new("PK\x03\x04 not a zip".b)
      rescue HyperTabular::TabularError => e
        print " ", e.class
      end
    RUBY
    out, status = Open3.capture2e(RbConfig.ruby, "-I", lib, "-r", "hypertabular", "-e", script)
    expect([status.success?, out]).to eq([true, "native [\"Data\"] HyperTabular::TabularError"])
  end
end
