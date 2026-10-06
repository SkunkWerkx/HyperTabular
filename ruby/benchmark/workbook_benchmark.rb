#!/usr/bin/env ruby
# frozen_string_literal: true

# benchmark-ips: the workbook reader over the two 300 000-row files of corpus/README.md —
# Excel's excel-win-300k.xlsx and LibreOffice's libreoffice-300k.ods, 8 columns each — read
# whole through the plan rust/benches/workbook_benchmarks.rs reads them through, so that this
# binding's number sits beside the core's. "open" is the package opened and nothing read.
# "values" is opened, then the first sheet read in batches of 4096, every column's values
# made — nil where a cell did not cast, so the cells that did are counted from them — and
# the text column's lengths summed: the checksum every binding's benchmark arrives at, which
# says they all did the same work. "verdicts" counts the cells that cast from every column's
# verdicts instead: a HyperCast::Success or Fault a cell, what a caller matching every cell
# with case/in pays.
#
# The files stay out of the tree (corpus/generate/out/, sha256 in the README's table);
# HYPERTABULAR_BENCH_DIR names another directory holding them. A file that is not there is
# skipped.
# Run: ruby benchmark/workbook_benchmark.rb

$LOAD_PATH.unshift(File.expand_path("../lib", __dir__))

require "benchmark/ips"
require "hypertabular"

PLAN = [
  HyperTabular::Column.i64(0), HyperTabular::Column.f64(1), HyperTabular::Column.text(2),
  HyperTabular::Column.date(3), HyperTabular::Column.time(4), HyperTabular::Column.bool(5),
  HyperTabular::Column.duration(6), HyperTabular::Column.i64(7)
].freeze
OPTIONS = HyperTabular::SheetOptions.new
DIRECTORY = ENV.fetch("HYPERTABULAR_BENCH_DIR") { File.expand_path("../../corpus/generate/out", __dir__) }

# Reads the first sheet whole, counting what cast from its values or from its verdicts.
def read(container, verdicts:)
  checksum = 0
  HyperTabular::Workbook.new(container).sheet(0, OPTIONS, PLAN).each_batch do |batch|
    PLAN.size.times do |column|
      checksum +=
        if verdicts
          batch.verdicts(column).count { |verdict| verdict.is_a?(HyperCast::Success) }
        else
          batch.values(column).count { |value| !value.nil? }
        end
    end
    batch.values(2).each { |text| checksum += text.bytesize if text }
  end
  checksum
end

%w[excel-win-300k.xlsx libreoffice-300k.ods].each do |name|
  path = File.join(DIRECTORY, name)
  unless File.exist?(path)
    puts "#{name}: not in #{DIRECTORY}, skipped"
    next
  end
  # Frozen, so that the workbook reads it in place rather than copying it each time.
  container = File.binread(path).freeze
  puts "#{name}: checksum #{read(container, verdicts: false)}"
  Benchmark.ips do |x|
    x.config(warmup: 2, time: 10)
    x.report("#{name} open") { HyperTabular::Workbook.new(container).sheets.size }
    x.report("#{name} values") { read(container, verdicts: false) }
    x.report("#{name} verdicts") { read(container, verdicts: true) }
  end
end
