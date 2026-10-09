require "digest"
require "stringio"
require_relative "corpus_helpers"

# What the cross-backend agreement spec compares: every cell of both corpora read through
# whichever backend this process runs on, said as text — each verdict as it inspects, the
# text the cell was cast from, the row's line, every header and every failure — and hashed.
# The spec computes it in its own process (the Magnus extension) and in a subprocess forced
# onto Fiddle, and the two must be the same string.
module Agreement
  extend CorpusHelpers

  # Every read in the delimited and workbook corpora, in batches of +batch_rows+, as one
  # SHA-256 hex digest.
  def self.digest(batch_rows = 3)
    sha = Digest::SHA256.new
    CorpusHelpers.corpus("delimited.json").each do |vector|
      settings = vector["dialect"]
      dialect = HyperTabular::Dialect.new(
        separator: settings["separator"], quoting: settings["quoting"],
        has_header: settings["has_header"], skip_blank_lines: settings["skip_blank_lines"]
      )
      plan = vector["plan"].map { |entry| column_of(entry) }
      read(sha, vector["name"]) do
        reader = HyperTabular::DelimitedReader.new(vector["input"], dialect, plan, batch_rows: batch_rows)
        sha << reader.header.inspect
        each_batch(sha, plan) { reader.read }
      end
    end
    CorpusHelpers.corpus("workbook.json").each do |vector|
      read(sha, vector["name"]) do
        book = HyperTabular::Workbook.open(File.join(CorpusHelpers.directory, vector["file"]))
        sha << [book.format, book.date_system, book.sheets].inspect
        next unless vector.key?("sheet")

        options = HyperTabular::SheetOptions.new(
          has_header: vector["options"]["has_header"], skip_empty_rows: vector["options"]["skip_empty_rows"],
          batch_rows: batch_rows
        )
        plan = vector["plan"].map { |entry| column_of(entry) }
        sheet = book.sheet(vector["sheet"], options, plan)
        sha << sheet.header.inspect
        each_batch(sha, plan) { sheet.read }
      end
    end
    sha.hexdigest
  end

  # The same reads header first — every reader and sheet opened without a plan and bound
  # to it once its header is read, every workbook opened from an IO — walked a Row at a
  # time, with the column count a reader reports before and after, as one SHA-256 hex digest.
  def self.header_first_digest(batch_rows = 2)
    sha = Digest::SHA256.new
    CorpusHelpers.corpus("delimited.json").each do |vector|
      settings = vector["dialect"]
      dialect = HyperTabular::Dialect.new(
        separator: settings["separator"], quoting: settings["quoting"],
        has_header: settings["has_header"], skip_blank_lines: settings["skip_blank_lines"]
      )
      plan = vector["plan"].map { |entry| column_of(entry) }
      read(sha, vector["name"]) do
        reader = HyperTabular::DelimitedReader.new(StringIO.new(vector["input"]), dialect,
                                                   batch_rows: batch_rows, buffer_bytes: 5)
        sha << [reader.header, reader.column_count].inspect
        reader.bind(plan)
        each_row(sha, reader)
        sha << reader.column_count.inspect
      end
    end
    CorpusHelpers.corpus("workbook.json").each do |vector|
      read(sha, vector["name"]) do
        book = File.open(File.join(CorpusHelpers.directory, vector["file"]), "rb") do |file|
          HyperTabular::Workbook.new(file)
        end
        sha << [book.format, book.date_system, book.sheets].inspect
        next unless vector.key?("sheet")

        options = HyperTabular::SheetOptions.new(
          has_header: vector["options"]["has_header"], skip_empty_rows: vector["options"]["skip_empty_rows"],
          batch_rows: batch_rows
        )
        sheet = book.sheet(vector["sheet"], options)
        sha << sheet.header.inspect
        each_row(sha, sheet.bind(vector["plan"].map { |entry| column_of(entry) }))
      end
    end
    sha.hexdigest
  end

  def self.each_row(sha, rows)
    rows.each do |row|
      sha << [row.index, row.line].inspect
      row.size.times { |column| sha << row.get(column).inspect << row.raw(column).inspect }
    end
  end

  def self.read(sha, name)
    sha << name
    yield
  rescue HyperTabular::TabularError => e
    sha << [e.kind, e.record, e.line, e.byte, e.message].inspect
  end

  def self.each_batch(sha, plan)
    while (batch = yield)
      batch.rows.times do |row|
        sha << batch.line(row).to_s
        plan.each_index do |column|
          sha << batch.get(column, row).inspect << batch.raw(column, row).inspect
        end
      end
    end
  end
end
