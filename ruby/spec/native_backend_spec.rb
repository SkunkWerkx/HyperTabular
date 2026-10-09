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

  it "agrees with the Fiddle backend on a sheet whose repeated header row a late plan reads" do
    path = File.expand_path("../../corpus/workbook/generated-a.ods", __dir__).dump
    script = "s = HyperTabular::Workbook.open(#{path}).sheet(1, HyperTabular::SheetOptions::DEFAULT); " \
             "s.bind(s.header.each_index.map { |i| HyperTabular::Column.text(i) }); s.map(&:to_a).inspect"
    expect(fiddle_eval(script)).to eq(eval(script))
  end

  it "agrees with the Fiddle backend on a fault span in multi-byte text" do
    script = 'r = HyperTabular::DelimitedReader.new("n\nééx\n", HyperTabular::Dialect::CSV, ' \
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
