require "digest"
require "stringio"
require_relative "corpus_helpers"

# What the cross-backend agreement spec compares: every cell of both corpora read through
# whichever backend this process runs on, said as text — each verdict as it inspects, the
# text the cell was cast from, where the verdict came from, the row's line, each batch's faults
# list, every suggestion, every header and every failure — and hashed.
# The spec computes it in its own process (the Magnus extension) and in a subprocess forced
# onto Fiddle, and the two must be the same string.
module Agreement
  extend CorpusHelpers

  # Every read in the delimited and workbook corpora, in batches of +batch_rows+, as one
  # SHA-256 hex digest.
  def self.digest(batch_rows = 3)
    sha = Digest::SHA256.new
    CorpusHelpers.corpus("delimited.json").each do |vector|
      dialect = dialect_of(vector["dialect"])
      plan = vector["plan"].map { |entry| column_of(entry) }
      read(sha, vector["name"]) do
        reader = HyperTabular::DelimitedReader.new(input_of(vector), dialect, plan, batch_rows: batch_rows)
        sha << reader.header.inspect
        each_batch(sha, plan) { reader.read }
      end
    end
    CorpusHelpers.corpus("workbook.json").each do |vector|
      read(sha, vector["name"]) do
        book = HyperTabular::Workbook.open(File.join(CorpusHelpers.directory, vector["file"]))
        sha << [book.format, book.date_system, book.sheets].inspect
        next unless vector.key?("sheet")

        options = options_of(vector["options"], batch_rows)
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
      dialect = dialect_of(vector["dialect"])
      plan = vector["plan"].map { |entry| column_of(entry) }
      read(sha, vector["name"]) do
        reader = HyperTabular::DelimitedReader.new(StringIO.new(input_of(vector)), dialect,
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

        options = options_of(vector["options"], batch_rows)
        sheet = book.sheet(vector["sheet"], options)
        sha << sheet.header.inspect
        each_row(sha, sheet.bind(vector["plan"].map { |entry| column_of(entry) }))
      end
    end
    sha.hexdigest
  end

  # Every suggestion both corpora have, and what every case without one says, as one SHA-256 hex
  # digest. A refusal hashes as its failure, an unrefused plan as what a report holds.
  def self.suggest_digest
    sha = Digest::SHA256.new
    CorpusHelpers.corpus("delimited.json").each do |vector|
      read(sha, vector["name"]) do
        asked = vector.key?("suggest") ? suggest_options(vector["suggest"]) : HyperTabular::SuggestOptions::DEFAULT
        sha << HyperTabular::DelimitedReader.suggest_string(input_of(vector), dialect_of(vector["dialect"]), asked)
                                            .inspect
      end
    end
    CorpusHelpers.corpus("workbook.json").each do |vector|
      next unless vector.key?("sheet")

      read(sha, vector["name"]) do
        book = HyperTabular::Workbook.open(File.join(CorpusHelpers.directory, vector["file"]))
        asked = vector.key?("suggest") ? suggest_options(vector["suggest"]) : HyperTabular::SuggestOptions::DEFAULT
        sha << book.sheet(vector["sheet"], options_of(vector["options"], 4096)).suggest(asked).inspect
      end
    end
    sha.hexdigest
  end

  def self.suggest_options(expected)
    HyperTabular::SuggestOptions.new(sample_rows: expected.fetch("sample_rows", 1000),
                                     format: expected.key?("format") ? format_of(expected) : nil)
  end

  def self.each_row(sha, rows)
    rows.each do |row|
      sha << [row.index, row.line].inspect
      row.size.times do |column|
        sha << row.to_a[column].inspect << row.raw(column).inspect << row.origin(column).inspect
      end
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
      sha << batch.faults.inspect
      batch.rows.times do |row|
        sha << batch.line(row).to_s
        plan.each_index do |column|
          verdict = batch.send(:verdict_of, column, row)
          sha << verdict.inspect << batch.raw(column, row).inspect << batch.origin(column, row).inspect
          sha << verdict.detail.inspect if verdict.is_a?(HyperCast::Fault)
        end
      end
    end
  end
end
