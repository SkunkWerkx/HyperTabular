# The surface the plan-later, row-at-a-time reading adds: a reader or a sheet opened without
# a plan and bound once its header has said where each column is, the Header that says it,
# a workbook from an IO, and the Row a batch, a reader and a sheet iterate. The corpus specs
# replay every case header first; these pin the contract around that.

require "pathname"
require "stringio"
require "tempfile"
require "spec_helper"
require_relative "corpus_helpers"

RSpec.describe "header first" do
  csv = HyperTabular::Dialect::CSV
  column = HyperTabular::Column
  reader_class = HyperTabular::DelimitedReader

  def basic_xlsx = File.join(CorpusHelpers.directory, "workbook", "basic.xlsx")

  describe HyperTabular::Header do
    let(:header) { reader_class.new("id,name,Name,name, name\n", csv).header }

    it "is still the frozen Array of frozen names it always was" do
      expect(header).to be_a(described_class).and be_frozen
      expect(header).to eq(["id", "name", "Name", "name", " name"])
      expect(["id", "name", "Name", "name", " name"]).to eq(header)
      expect(header).to all(be_frozen)
      expect(header.first(2)).to eq(%w[id name])
      expect(header.inspect).to eq('["id", "name", "Name", "name", " name"]')
    end

    it "finds a name exactly, case and spaces included, and a repeated one where it is first" do
      expect(header.ordinal("id")).to eq(0)
      expect(header.ordinal("name")).to eq(1)
      expect(header.ordinal("Name")).to eq(2)
      expect(header.ordinal(" name")).to eq(4)
      expect(header.find_ordinal("NAME")).to be_nil
      expect(header.find_ordinal("name ")).to be_nil
      expect(header.find_ordinal("name")).to eq(1)
    end

    it "names the column it does not have, unless a block says what to do instead" do
      expect { header.ordinal("M49 Code") }.to raise_error(KeyError, /"M49 Code"/) { |e|
        expect([e.key, e.receiver]).to eq(["M49 Code", header])
      }
      expect(header.ordinal("M49 Code") { |name| "no #{name}" }).to eq("no M49 Code")
      expect { header.ordinal(:id) }.to raise_error(TypeError)
    end

    it "matches a binary name as its bytes and another encoding as its UTF-8" do
      header = reader_class.new("Größe,x\n", csv).header
      expect(header.ordinal("Größe".b)).to eq(0)
      expect(header.ordinal("Größe".encode(Encoding::ISO_8859_1))).to eq(0)
      expect(header.find_ordinal("Gr\xFF".b)).to be_nil
    end
  end

  describe HyperTabular::DelimitedReader do
    it "opens, reads the header, binds a plan built from it, and reads" do
      reader = reader_class.new("id,name,score\n1,alice,2.5\n2,bob,x\n", csv)
      expect([reader.bound?, reader.plan, reader.column_count]).to eq([false, [], 3])
      expect(reader.inspect).to include("unbound")
      header = reader.header
      plan = [column.f64(header.ordinal("score")), column.text(header.ordinal("name"))]
      expect(reader.bind(plan)).to equal(reader)
      expect([reader.bound?, reader.plan]).to eq([true, plan])
      batch = reader.read
      expect([batch.values(0), batch.values(1)]).to eq([[2.5, nil], %w[alice bob]])
      expect(reader.read).to be_nil
    end

    it "refuses a read before its plan, without holding it against the reader" do
      reader = reader_class.new("n\n1\n2\n", csv, batch_rows: 1)
      expect { reader.read }.to raise_error(RuntimeError, /no plan yet/)
      expect { reader.each_row.to_a }.to raise_error(RuntimeError, /no plan yet/)
      reader.bind([column.i32(0)])
      expect(reader.each_row.to_a).to eq([[HyperCast::Success.new(value: 1)], [HyperCast::Success.new(value: 2)]])
    end

    it "binds once, and a refused plan leaves it unbound" do
      reader = reader_class.new("n\n1\n", csv)
      expect { reader.bind([:not_a_column]) }.to raise_error(ArgumentError, /plan column 0/)
      expect(reader).not_to be_bound
      reader.bind([column.i32(0)])
      expect { reader.bind([column.i32(0)]) }.to raise_error(RuntimeError, /already has a plan/)
      expect { reader_class.new("n\n1\n", csv, [column.i32(0)]).bind([]) }.to raise_error(RuntimeError)
      reader.close
      expect { reader_class.new("n\n", csv).tap(&:close).bind([]) }.to raise_error(IOError)
    end

    it "binds positional ordinals where there is no header, and counts the first record's cells" do
      reader = reader_class.new("1,2,3\n4,5,6\n", csv.with(has_header: false), batch_rows: 1)
      expect([reader.header, reader.column_count]).to eq([nil, nil])
      reader.bind([column.i32(2)])
      expect(reader.read.values(0)).to eq([3])
      expect(reader.column_count).to eq(3)
    end

    it "keeps the plan's place in line: a plan handed over is bound before the header is read" do
      expect { reader_class.new("\"open", csv, [:nope]) }.to raise_error(ArgumentError)
      expect { reader_class.new("\"open", csv) }.to raise_error(HyperTabular::TabularError)
    end

    it "opens a path header first, with or without a block" do
      Tempfile.create(["header", ".csv"]) do |file|
        file.write("a,b\n1,2\n")
        file.close
        reader = reader_class.open(file.path, csv)
        expect(reader.bind([column.i32(reader.header.ordinal("b"))]).read.values(0)).to eq([2])
        reader.close
        values = reader_class.open(file.path, csv) { |r| r.bind([column.i32(0)]).map { |row| row.value(0) } }
        expect(values).to eq([1])
      end
    end

    it "iterates rows across batches, and stops at the end" do
      text = "n,t\n#{(1..10).map { |n| "#{n},r#{n}" }.join("\n")}\n"
      reader = reader_class.new(text, csv, [column.i32(0), column.text(1)], batch_rows: 3)
      rows = reader.each.to_a
      expect(rows.map { |row| row.value(0) }).to eq((1..10).to_a)
      expect(rows.map(&:line)).to eq((2..11).to_a)
      expect(rows.map(&:index)).to eq([0, 1, 2, 0, 1, 2, 0, 1, 2, 0])
      expect(rows.map(&:batch).uniq.size).to eq(4)
      # Every row outlives the read that delivered it: its batch is a copy.
      expect(rows.first.raw(1)).to eq("r1")
      expect(reader.read).to be_nil
      expect(reader.to_a).to eq([])
      expect(reader_class.new(text, csv, [column.i32(0)], batch_rows: 4).count).to eq(10)
    end
  end

  describe HyperTabular::Row do
    it "answers every member as the batch does for its row" do
      reader = reader_class.new("n,t\n1,a\nx,\"b\"\"c\"\n\n3,\n", csv.with(skip_blank_lines: false),
                                [column.i32(0), column.text(1)])
      batch = reader.read
      expect(batch.count).to eq(batch.rows)
      expect(batch.each).to be_a(Enumerator).and have_attributes(size: batch.rows)
      batch.each.with_index do |row, index|
        expect(row).to be_frozen
        expect([row.index, row.line, row.size]).to eq([index, batch.line(index), 2])
        2.times do |col|
          expect(row.get(col)).to eq(batch.get(col, index))
          expect(row[col]).to eq(batch.get(col, index))
          expect(row.verdict(col)).to eq(batch.get(col, index))
          expect(row.value(col)).to eq(batch.values(col)[index])
          expect(row.raw(col)).to eq(batch.raw(col, index))
        end
        expect(row.to_a).to eq(batch.columns.each_index.map { |col| batch.get(col, index) })
        expect(batch.row(index).to_a).to eq(row.to_a)
      end
      expect { batch.row(batch.rows) }.to raise_error(IndexError)
      expect { batch.first.get(2) }.to raise_error(IndexError)
    end

    it "is the row DelimitedReader#each_row yields, and matches as its verdicts" do
      text = "n,t\n1,a\nx,b\n"
      plan = [column.i32(0), column.text(1)]
      expect(reader_class.new(text, csv, plan).map(&:to_a)).to eq(reader_class.new(text, csv, plan).each_row.to_a)
      row = reader_class.new(text, csv, plan).first
      matched =
        case row
        in [HyperCast::Success(value: Integer => id), HyperCast::Success(value: String => name)] then [id, name]
        end
      expect(matched).to eq([1, "a"])
      expect(row.inspect).to eq("#<HyperTabular::Row index=0 line=2>")
    end
  end

  describe HyperTabular::Workbook do
    it "opens a sheet header first, by index and by name, and binds it" do
      book = described_class.open(basic_xlsx)
      [0, book.sheets.first.name].each do |which|
        sheet = book.sheet(which, HyperTabular::SheetOptions::DEFAULT)
        expect([sheet.bound?, sheet.plan]).to eq([false, []])
        expect(sheet.inspect).to include("unbound")
        expect(sheet.header).to be_a(HyperTabular::Header).and be_frozen
        expect { sheet.read }.to raise_error(RuntimeError, /no plan yet/)
        expect { sheet.bind(["id"]) }.to raise_error(ArgumentError)
        expect(sheet.bind([column.i64(sheet.header.ordinal("id"))])).to equal(sheet)
        expect { sheet.bind([column.i64(0)]) }.to raise_error(RuntimeError, /already has a plan/)
        planned = book.sheet(which, HyperTabular::SheetOptions::DEFAULT, sheet.plan)
        expect(sheet.map(&:to_a)).to eq(planned.map(&:to_a)).and have_attributes(size: 4)
      end
      expect { book.sheet(0, HyperTabular::SheetOptions::DEFAULT).header.ordinal("nope") }
        .to raise_error(KeyError, /"nope"/)
    end

    it "iterates a sheet's rows across batches" do
      sheet = described_class.open(basic_xlsx).sheet(0, HyperTabular::SheetOptions.new(batch_rows: 1),
                                                     [column.i64(0)])
      rows = sheet.each.to_a
      expect(rows.map(&:line)).to eq([2, 3, 4, 6])
      expect(rows.map(&:batch).uniq.size).to eq(4)
      expect(sheet.read).to be_nil
    end

    it "reads an IO to its end, and closes it only when told to" do
      bytes = File.binread(basic_xlsx)
      kept = StringIO.new(bytes.dup)
      expect(described_class.new(kept).sheets.map(&:name)).to eq(described_class.new(bytes).sheets.map(&:name))
      expect([kept.closed?, kept.eof?]).to eq([false, true])
      closed = StringIO.new(bytes.dup)
      expect(described_class.new(closed, close_source: true).format).to eq(:xlsx)
      expect(closed).to be_closed
      broken = StringIO.new("not a zip")
      expect { described_class.new(broken, close_source: true) }.to raise_error(HyperTabular::TabularError)
      expect(broken).to be_closed
      expect { described_class.new(Pathname.new(basic_xlsx)) }.to raise_error(ArgumentError, /Pathname/)
      expect { described_class.new(42) }.to raise_error(ArgumentError, /String or an IO/)
    end
  end
end
