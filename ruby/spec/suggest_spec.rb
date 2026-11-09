# A suggestion: what a sample of a source says of each column, a plan to inspect and hand on,
# never applied. The corpus replays hold what it says of its cases; this holds the rest.

require "tempfile"
require "stringio"
require "spec_helper"
require_relative "corpus_helpers"
require_relative "package_helpers"

RSpec.describe "suggest" do
  column = HyperTabular::Column
  reader = HyperTabular::DelimitedReader
  workbook = HyperTabular::Workbook
  options_class = HyperTabular::SuggestOptions
  sheet_options = HyperTabular::SheetOptions
  csv = HyperTabular::Dialect::CSV
  invariant = HyperCast::NumFormat::INVARIANT
  comma = HyperCast::NumFormat.new(decimal_sep: ",", group_sep: ".", flags: HyperCast::ALL_STYLES)
  indian = invariant.with(grouping: :indian)
  workbooks = File.join(CorpusHelpers.directory, "workbook")

  define_method(:suggest) do |text, asked = options_class::DEFAULT, dialect = csv|
    reader.suggest_string(text, dialect, asked)
  end
  define_method(:doors) { |found| found.report.map(&:door) }
  define_method(:rows_of) { |count| "a\n#{(2..count + 1).map { |row| "#{row}\n" }.join}" }
  define_method(:one) { |text, plan, dialect = csv| reader.new(text, dialect, plan).read }
  define_method(:book) { |file| workbook.open(File.join(workbooks, file)) }
  define_method(:column_named) do |file, name|
    found = book(file).sheet(0, sheet_options::DEFAULT).suggest.report.find { |said| said.name == name }
    [found.door, found.status]
  end
  define_method(:same) { |found, wanted| expect(found.packed).to eq(wanted.packed) }

  it "says what the plan does and why" do
    found = suggest("flag,id,big,score,day,at,key,name,mixed,sep,order,none\n" \
                    "true,1,5000000000,2.5,2026-01-07,2026-01-07T00:00:00Z,6ba7b810-9dad-11d1-80b4-00c04fd430c8," \
                    "abc,1,\"1,234\",01/02/2026,\n" \
                    "false,2,6000000000,3,2026-01-08,2026-01-08T00:00:00Z,6ba7b811-9dad-11d1-80b4-00c04fd430c8," \
                    "def,x,\"2,345\",03/04/2026,\n")
    expect(doors(found)).to eq(%i[bool i32 i64 f64 date timestamp uuid text text i32 date_ordered text])
    expect(found.report[10].declared).to eq(:month_day_year)
    # Only what's resolved is read through a door: the last four read as text.
    expect(found.plan.map(&:door)).to eq(%i[bool i32 i64 f64 date timestamp uuid text text text text text])
    expect(found.report.map(&:status))
      .to eq([:resolved] * 8 + %i[unresolved ambiguous ambiguous unresolved])
    found.report.each_with_index do |said, ordinal|
      expect([said.ordinal, found.plan[ordinal].ordinal]).to eq([ordinal, ordinal])
    end
    expect([found.report[0].name, found.report[11].name]).to eq(%w[flag none])
    expect([found.report[8].matched, found.report[8].sampled]).to eq([1, 2])
    expect([found.report[11].matched, found.report[11].sampled]).to eq([0, 0])
    expect([found.report[9].matched, found.report[9].sampled]).to eq([2, 2])
    expect(found.report[0].candidates).to eq([])
    expect(found.report[10].candidates).to eq(%i[month_day_year day_month_year])
    first, second = found.report[9].candidates
    same(first, invariant)
    expect([second.decimal_sep, second.group_sep]).to eq([",", "."])
    expect(found.truncated).to be(false)
  end

  it "plans a read of what was sampled" do
    text = "flag,id,score,day,order,name\n1,2,2.5,2026-01-07,13/02/2026,abc\n0,3,3,2026-01-08,01/02/2026,def\n"
    batch = one(text, suggest(text).plan)
    expect([batch.rows, batch.faults]).to eq([2, []])
    expect(batch.values(0)).to eq([true, false])
    expect(batch.values(1)).to eq([2, 3])
    expect(batch.values(2)).to eq([2.5, 3.0])
    expect(batch.values(5)[1]).to eq("def")
  end

  HyperTabular::DOORS.each do |door, code|
    it "makes a column that binds of every door the core can name: #{door}" do
      declares = HyperTabular::Column::DECLARES[door]
      (declares ? declares.values : [0]).each do |param|
        made = column.of_code(0, code, param, invariant)
        expect(made.door).to eq(door)
        expect(made.declared).to eq(declares&.key(param))
        expect(reader.new("a\n1\n", csv, [made]).read).not_to be_nil
      end
    end
  end

  it "has no column for a door or a declaration this version doesn't know" do
    expect(column.of_code(0, 99, 0)).to be_nil
    expect(column.of_code(0, HyperTabular::DOORS.fetch(:unix), 99)).to be_nil
  end

  it "reaches the plan with a declared format" do
    text = %(a,b\n"1,234","1,234"\n)
    found = suggest(text, options_class.new(format: invariant, column_formats: { 1 => comma }))
    expect(doors(found)).to eq(%i[i32 f64])
    expect(found.plan[1].format.decimal_sep).to eq(",")
    batch = one(text, found.plan)
    expect([batch.values(0), batch.values(1)]).to eq([[1234], [1.234]])
  end

  it "takes the last format declared for a column" do
    found = suggest(%(a\n"1,234"\n), options_class.new(column_formats: [[0, invariant], [0, comma]]))
    expect(doors(found)).to eq([:f64])
    # What the caller declared comes back as the caller's own object.
    expect(found.report[0].format).to equal(comma)
    expect(found.plan[0].format).to equal(comma)
    found = suggest(%(a\n"1,23,456"\n), options_class.new(column_formats: [[0, invariant], [0, indian]]))
    expect(doors(found)).to eq([:i32])
    expect(found.report[0].format).to equal(indian)
    # A column's own is ahead of everything's, and everything's is the rest's.
    found = suggest(%(a,b\n"1,23,456","1,234,567"\n),
                    options_class.new(format: indian, column_formats: { 1 => invariant }))
    expect(doors(found)).to eq(%i[i32 i32])
    expect(found.report[0].format).to equal(indian)
    same(found.report[1].format, invariant)
  end

  it "refuses a format the core would, standing in for an old program's bytes" do
    good = invariant.packed[20, 9]
    packing = lambda do |group|
      packed = invariant.packed.dup
      packed[20, 9] = group
      Class.new(HyperCast::NumFormat) { define_method(:packed) { packed } }
           .new(decimal_sep: ".", group_sep: ",", flags: HyperCast::ALL_STYLES)
    end
    expect(packing.call(good).packed).to eq(invariant.packed)
    expect { suggest("a,b\n1,2\n", options_class.new(column_formats: { 1 => packing.call(good) })) }.not_to raise_error
    expect { suggest("a,b\n1,2\n", options_class.new(column_formats: { 1 => packing.call(".\0\0\0\0\0\0\0\x01".b) })) }
      .to raise_error(ArgumentError, /plan column 1 cannot be honoured/)
    expect { suggest("a\n0.5\n", options_class.new(format: packing.call("\0".b * 9))) }
      .to raise_error(ArgumentError, /cannot be honoured/)
  end

  it "checks options where they are made" do
    [-1, 1.5, "1", nil].each { |rows| expect { options_class.new(sample_rows: rows) }.to raise_error(ArgumentError) }
    expect { options_class.new(column_formats: [1]) }.to raise_error(ArgumentError, /pairs/)
    expect { options_class.new(column_formats: [[-1, comma]]) }.to raise_error(ArgumentError, /pairs/)
    expect { options_class.new(column_date_orders: [[0, :whenever]]) }.to raise_error(KeyError)
    expect { options_class.new(date_order: :whenever) }.to raise_error(KeyError)
    expect { reader.suggest_string("a\n", csv, Object.new) }.to raise_error(ArgumentError, /SuggestOptions/)
    sheet = book("basic.xlsx").sheet(0, sheet_options::DEFAULT)
    expect { sheet.suggest({}) }.to raise_error(ArgumentError, /SuggestOptions/)
  end

  it "applies nothing" do
    text = "id,score\n1,2.5\n2,3.5\n"
    Tempfile.create(["a", ".csv"]) do |file|
      file.write(text)
      file.close
      expect(doors(reader.suggest(file.path, csv))).to eq(%i[i32 f64])
      declared = [column.text(0), column.text(1)]
      held = reader.open(file.path, csv, declared)
      expect(held.plan).to eq(declared)
      batch = held.read
      expect([batch.values(0)[0], batch.values(1)[0]]).to eq(["1", "2.5"])
      held.close
    end
    unbound = reader.new(text, csv)
    suggest(text)
    expect(unbound).not_to be_bound
    expect(unbound.plan).to eq([])
    expect { unbound.read }.to raise_error(RuntimeError, /no plan/)
    sheet = book("basic.xlsx").sheet(0, sheet_options::DEFAULT)
    sheet.suggest
    expect(sheet).not_to be_bound
    expect(sheet.plan).to eq([])
    expect { sheet.read }.to raise_error(RuntimeError, /no plan/)
  end

  it "answers the same of a bound or part-read sheet, and leaves it where it is" do
    opened = book("libreoffice-data.xlsx")
    options = sheet_options.new(batch_rows: 1)
    asked = options_class.new(sample_rows: 1)
    fresh = opened.sheet(0, options).suggest(asked)
    plan = fresh.plan.each_index.map { |at| column.text(at) }
    rows = opened.sheet(0, options, plan).each_batch.map { |batch| batch.values(0)[0] }
    expect(rows.size).to be > 1

    sheet = opened.sheet(0, options, plan)
    expect(sheet.suggest(asked)).to eq(fresh)
    expect(sheet.read.values(0)[0]).to eq(rows[0])
    expect(sheet.suggest(asked)).to eq(fresh)
    expect(sheet.plan).to eq(plan)
    expect(sheet.read.values(0)[0]).to eq(rows[1])
  end

  it "samples the sheet that was named" do
    opened = book("multisheet.xlsx")
    answered = 0
    opened.sheets.each_with_index do |info, at|
      begin
        by_index = opened.sheet(at, sheet_options::DEFAULT)
      rescue HyperTabular::TabularError
        expect { opened.sheet(info.name, sheet_options::DEFAULT) }.to raise_error(HyperTabular::TabularError)
        next
      end
      expect(by_index.suggest).to eq(opened.sheet(info.name, sheet_options::DEFAULT).suggest)
      answered += 1
    end
    expect(answered).to be > 1
  end

  { 2 => false, 3 => false, 4 => true, 5 => true, 9000 => true }.each do |rows, truncated|
    it "says a sample of 3 from #{rows} rows #{truncated ? 'is' : 'is not'} truncated" do
      exact = options_class.new(sample_rows: 3)
      found = suggest(rows_of(rows), exact)
      expect(found.truncated).to be(truncated)
      expect(found.report[0].sampled).to eq([rows, 3].min)
      Tempfile.create(["exact", ".csv"]) do |file|
        file.write(rows_of(rows))
        file.close
        expect(reader.suggest(file.path, csv, exact).truncated).to be(truncated)
      end
    end
  end

  it "samples a thousand rows, and a batch at most" do
    text = rows_of(9000)
    default = suggest(text)
    expect([default.report[0].sampled, default.truncated]).to eq([1000, true])
    most = options_class.new(sample_rows: 1_000_000)
    found = suggest(text, most)
    expect([found.report[0].sampled, found.truncated]).to eq([4096, true])
    found = suggest(rows_of(4096), most)
    expect([found.report[0].sampled, found.truncated]).to eq([4096, false])
  end

  it "takes a sample of none as one, and a declaration alone leaves the thousand" do
    text = "a\n1\n2.5\n"
    found = suggest(text, options_class.new(sample_rows: 0))
    expect(doors(found)).to eq([:bool])
    expect([found.report[0].sampled, found.truncated]).to eq([1, true])
    found = suggest(text)
    expect(doors(found)).to eq([:f64])
    expect([found.report[0].sampled, found.truncated]).to eq([2, false])
    found = suggest(text, options_class.new(format: invariant))
    expect([found.report[0].sampled, found.truncated]).to eq([2, false])
    found = suggest(text, options_class.new(date_order: :day_month_year))
    expect([found.report[0].sampled, found.truncated]).to eq([2, false])
    sheet = book("basic.xlsx").sheet(0, sheet_options::DEFAULT)
    expect(sheet.suggest(options_class.new(sample_rows: 0)).report[0].sampled).to eq(1)
    expect(sheet.suggest(options_class.new(format: invariant)).report[0].sampled).to be > 1
  end

  it "gives what the sample did not reach no say" do
    text = "#{rows_of(3)}oops\n"
    found = suggest(text, options_class.new(sample_rows: 2))
    expect(doors(found)).to eq([:i32])
    expect([found.report[0].status, found.truncated]).to eq([:resolved, true])
    expect(suggest(text).report[0].status).to eq(:unresolved)
  end

  it "takes a sheet's sample as its batch at most" do
    found = book("libreoffice-data.xlsx").sheet(0, sheet_options.new(batch_rows: 1)).suggest
    expect(found.truncated).to be(true)
    expect(found.report.map(&:sampled)).to all(be <= 1)
  end

  it "says a sheet of eight rows at batches of five is cut" do
    rows = (1..8).map { |row| %(<row r="#{row}"><c><v>#{row}</v></c></row>) }.join
    opened = workbook.new(PackageHelpers.xlsx(rows))
    found = opened.sheet(0, sheet_options.new(has_header: false, batch_rows: 5)).suggest
    expect([found.truncated, found.report[0].sampled]).to eq([true, 5])
    whole = opened.sheet(0, sheet_options.new(has_header: false)).suggest
    expect([whole.truncated, whole.report[0].sampled]).to eq([false, 8])
  end

  it "says a failure among the sampled rows, and not one past them" do
    text = "a,b\n1,2\n3,4\n5\n7,8\n"
    expect { suggest(text) }.to raise_error(HyperTabular::TabularError) { |e| expect(e.kind).to eq(:column_count) }
    found = suggest(text, options_class.new(sample_rows: 2))
    expect([found.truncated, found.report[0].sampled]).to eq([true, 2])
    # Declared, a ragged row is a read like any other, and what a row hasn't got is blank.
    padded = HyperTabular::Dialect.new(separator: ",", short_rows: :pad)
    expect(suggest(text, options_class::DEFAULT, padded).report[1].sampled).to eq(3)
    long = HyperTabular::Dialect.new(separator: ",", long_rows: :truncate)
    expect(suggest("a,b\n1,2,3\n", options_class::DEFAULT, long).plan.size).to eq(2)
  end

  it "takes a column of tokens for a column of blanks" do
    dialect = HyperTabular::Dialect.new(separator: ",", tokens: %w[NA -])
    found = suggest(%(a,b,c\nNA,1,"NA"\n-,NA,"NA"\nNA,3,"NA"\n), options_class::DEFAULT, dialect)
    expect(found.report[0].status).to eq(:unresolved)
    expect([found.report[0].matched, found.report[0].sampled, found.plan[0].door]).to eq([0, 0, :text])
    expect([found.report[1].door, found.report[1].status]).to eq(%i[i32 resolved])
    expect([found.report[1].matched, found.report[1].sampled]).to eq([2, 2])
    # A quoted cell is never a token, so it's the text NA.
    expect([found.report[2].door, found.report[2].status, found.report[2].sampled]).to eq([:text, :resolved, 3])
  end

  it "takes invalid text for a failed cell and not a blank" do
    found = suggest("a,b\n\xE9t\xE9,1\nabc,\xFF\n".b)
    expect(found.report[0].status).to eq(:unresolved)
    expect([found.report[0].matched, found.report[0].sampled]).to eq([0, 2])
    expect([found.report[1].matched, found.report[1].sampled]).to eq([1, 2])
  end

  it "judges a quote's doubled text" do
    expect(doors(suggest(%(a\n"1"""\n)))).to eq([:text])
    expect(doors(suggest(%(a\n"1"\n"2"\n)))).to eq([:i32])
  end

  it "has no names with no header, and its first record's width" do
    found = suggest("1,x\n2,y\n", options_class::DEFAULT, HyperTabular::Dialect.new(separator: ",", has_header: false))
    expect(doors(found)).to eq(%i[i32 text])
    expect([found.report[0].name, found.report[0].sampled]).to eq([nil, 2])
  end

  it "has nothing to plan of nothing" do
    [csv, HyperTabular::Dialect.new(separator: ",", has_header: false)].each do |dialect|
      found = suggest("", options_class::DEFAULT, dialect)
      expect([found.plan, found.report, found.truncated]).to eq([[], [], false])
    end
    found = suggest("a,b\n")
    expect(doors(found)).to eq(%i[text text])
    expect([found.report[0].status, found.report[0].sampled]).to eq([:unresolved, 0])
  end

  it "samples as the dialect declares" do
    dialect = HyperTabular::Dialect.new(separator: "\t", skip: 1, header_row: 1)
    found = suggest("a title, with a comma\nignored\tcolumns\nid\tscore\n7\t2.5\n", options_class::DEFAULT, dialect)
    expect(doors(found)).to eq(%i[i32 f64])
    expect(found.report[1].name).to eq("score")
    # Latin-1, decoded before anything's judged, and bytes and a file say the same.
    latin = HyperTabular::Dialect.new(separator: ",", encoding: :latin1)
    text = "caf\xE9\n7\n8\n".b
    found = suggest(text, options_class::DEFAULT, latin)
    expect([found.report[0].name, found.report[0].door]).to eq(["café", :i32])
    Tempfile.create(["latin1", ".csv"]) do |file|
      file.binmode
      file.write(text)
      file.close
      expect(reader.suggest(file.path, latin)).to eq(found)
    end
    # An encoding failure among the sample is the sample's.
    utf16 = HyperTabular::Dialect.new(separator: ",", encoding: :utf16le)
    expect { suggest("a\0\n\0\0\x001\0\n\0\0\xD8".b, options_class::DEFAULT, utf16) }
      .to raise_error(HyperTabular::TabularError) { |e| expect(e.kind).to eq(:encoding) }
  end

  it "says a path that is not there as the IO error it is" do
    expect { reader.suggest("/no/such/file.csv", csv) }.to raise_error(Errno::ENOENT)
  end

  it "has no names for a sheet with no header, and its first row's width" do
    found = book("basic.xlsx").sheet(0, sheet_options.new(has_header: false)).suggest
    expect([found.plan.size, found.report[0].name]).to eq([9, nil])
    # The header row is a row here: `id` and its four numbers, one door taking four of five.
    expect([found.report[0].door, found.report[0].matched, found.report[0].sampled]).to eq([:text, 4, 5])
  end

  it "judges a typed cell by what the text door says of it" do
    %w[basic.xlsx libreoffice-data.xlsx libreoffice-data.ods numbers-data.numbers].each do |file|
      expect(column_named(file, "id")).to eq(%i[i32 resolved]), file
      expect(column_named(file, "flag")).to eq(%i[bool resolved]), file
    end
    # A date, a date-time and a time of day, each as its writer's canonical text.
    %w[basic.xlsx libreoffice-data.ods numbers-data.numbers].each do |file|
      %w[day when at].each { |name| expect(column_named(file, name)).to eq(%i[text resolved]), "#{file}: #{name}" }
    end
    expect(column_named("libreoffice-data.ods", "elapsed")).to eq(%i[text resolved])
    expect(column_named("basic.xlsx", "amount")).to eq(%i[f64 resolved])
    expect(column_named("numbers-data.numbers", "pct")).to eq(%i[f64 resolved])
    expect(column_named("libreoffice-data.ods", "pct")).to eq(%i[f64 resolved])
    expect(column_named("basic.ods", "money")).to eq(%i[i32 resolved])
    # A column that mixes kinds says so, with a formula's cell an empty one.
    expect(column_named("numbers-data.numbers", "calc")).to eq(%i[text unresolved])
    expect(column_named("basic.ods", "stamp")).to eq(%i[text unresolved])
  end

  it "plans nothing for any sheet that refuses a cell it sampled, under each thing a caller can declare" do
    planned = typed = refused_by_suggest = 0
    read_fails = lambda do |opened, at|
      opened.sheet(at, sheet_options::DEFAULT, [column.text(0)]).each_batch { nil }
      false
    rescue HyperTabular::TabularError
      true
    end
    Dir.children(workbooks).sort.each do |file|
      path = File.join(workbooks, file)
      next if file.end_with?(".py") || !File.file?(path)

      begin
        opened = workbook.open(path)
      rescue HyperTabular::TabularError, ArgumentError
        next
      end
      opened.sheets.each_index do |at|
        [nil, invariant, comma, indian].each do |declared|
          begin
            sheet = opened.sheet(at, sheet_options::DEFAULT)
          rescue HyperTabular::TabularError
            next
          end
          begin
            found = sheet.suggest(options_class.new(format: declared))
          rescue HyperTabular::TabularError => e
            expect(read_fails.call(opened, at)).to be(true), "#{file} #{at}: #{e.message}"
            refused_by_suggest += 1
            next
          end
          next if found.plan.empty?

          planned += 1
          typed += found.plan.count { |planned_column| planned_column.door != :text }
          rows = 0
          opened.sheet(at, sheet_options::DEFAULT, found.plan).each_batch do |batch|
            refused = batch.faults.select do |problem|
              problem.row + rows < 1000 && %i[malformed out_of_range].include?(problem.reason) &&
                found.plan[problem.plan_column || 0].door != :text
            end
            expect(refused).to eq([]), "#{file} #{at}, #{declared.inspect}"
            rows += batch.rows
            break if rows >= 1000
          end
        end
      end
    end
    expect([planned >= 80, typed >= 350, refused_by_suggest >= 1]).to eq([true, true, true])
  end

  { "basic.xlsx" => %w[amount ratio], "basic.ods" => %w[amount pct],
    "excel-web-data.xlsx" => %w[pct money] }.each do |file, names|
    it "finds #{file}'s numbers numeric under any format" do
      [nil, invariant, comma].each do |asked|
        found = book(file).sheet(0, sheet_options::DEFAULT).suggest(options_class.new(format: asked))
        names.each do |name|
          said = found.report.find { |entry| entry.name == name }
          expect(said.status).to eq(:resolved), "#{file} #{name}"
          expect(HyperTabular::Column::NUMERIC).to include(said.door)
        end
      end
    end
  end

  %w[2147483647 2147483648 -2147483648 -2147483649 9223372036854775807 -9223372036854775808 9223372036854775808
     18446744073709551616 1e21 1.5E300 1.7976931348623157E+308 5e-324 -0 0.1 1234.5 NaN Infinity 1e999].each do |value|
    it "reads a typed #{value} through the door it suggests" do
      opened = workbook.new(PackageHelpers.xlsx(%(<row r="1"><c><v>#{value}</v></c></row>)))
      options = sheet_options.new(has_header: false)
      [nil, invariant, comma].each do |asked|
        found = opened.sheet(0, options).suggest(options_class.new(format: asked))
        batch = opened.sheet(0, options, found.plan).read
        expect(batch.faults.select { |problem| %i[malformed out_of_range].include?(problem.reason) }).to eq([]), value
        expect(found.report[0].status).not_to eq(:ambiguous), value
      end
    end
  end

  it "keeps a sheet's sample's notes, whatever a read of the sheet was to have" do
    opened = book("suggest.xlsx")
    found = opened.sheet(0, sheet_options::DEFAULT).suggest
    expect(found.report.first(3).map(&:status)).to eq(%i[resolved ambiguous ambiguous])
    expect(opened.sheet(0, sheet_options.new(notes: false)).suggest).to eq(found)
  end

  it "finds the owner's rule on a text cell in a sheet with notes off, and a typed UUID's second door" do
    rows = %(<row r="1"><c t="inlineStr"><is><t>1,234</t></is></c></row>)
    opened = workbook.new(PackageHelpers.xlsx(rows))
    found = opened.sheet(0, sheet_options.new(has_header: false, notes: false)).suggest
    expect(found.report[0].status).to eq(:ambiguous)
    expect(found.report[0].candidates.size).to eq(2)
  end

  it "settles with a declared date order a date the sample left open" do
    opened = "day\n01/02/2026\n03/04/2026\n"
    found = suggest(opened)
    expect([found.report[0].status, found.plan[0].door]).to eq(%i[ambiguous text])
    found = suggest(opened, options_class.new(date_order: :day_month_year))
    expect([found.report[0].status, found.report[0].candidates]).to eq([:resolved, []])
    expect([found.plan[0].door, found.plan[0].declared, found.report[0].declared])
      .to eq(%i[date_ordered day_month_year day_month_year])
    # A column's own wins, the last one given.
    found = suggest(opened, options_class.new(date_order: :day_month_year,
                                              column_date_orders: [[0, :day_month_year], [0, :month_day_year]]))
    expect(found.plan[0].declared).to eq(:month_day_year)
    # An order the column's cells rule out doesn't overrule them.
    found = suggest("day\n13/02/2026\n", options_class.new(date_order: :month_day_year))
    expect(found.plan[0].declared).to eq(:day_month_year)
    # An order out of the question's set, as year-first is, leaves the question open.
    found = suggest(opened, options_class.new(date_order: :year_month_day))
    expect([found.report[0].status, found.plan[0].door]).to eq(%i[ambiguous text])
  end

  it "keeps Western in the plan a comma that proved to group" do
    text = %(a\n"1,234"\n2.5\n"1,23,456"\n)
    found = suggest(text, options_class.new(sample_rows: 2))
    expect([found.report[0].door, found.report[0].status]).to eq(%i[f64 resolved])
    same(found.report[0].format, invariant)
    same(found.plan[0].format, invariant)
    batch = one(text, found.plan)
    expect(batch.values(0).first(2)).to eq([1234.0, 2.5])
    expect(batch.get(0, 2).reason).to eq(:malformed)
    # A month-day-year date's parameter is the same 2, and says nothing of a notation.
    found = suggest("a\n02/13/2026\n")
    expect([found.report[0].declared, found.report[0].format]).to eq([:month_day_year, nil])
    # Declared, it's the declared format, and the report names that.
    found = suggest(%(a\n"1,23,456"\n), options_class.new(format: indian))
    same(found.report[0].format, indian)
    same(found.plan[0].format, indian)
    expect(one(%(a\n"1,23,456"\n), found.plan).get(0, 0)).to eq(HyperCast::Success.new(value: 123_456))
    # A column that isn't a resolved number names no format.
    expect(suggest("a\nx\n").report[0].format).to be_nil
    found = suggest(%(a\n"1,234"\n))
    expect([found.report[0].status, found.report[0].format]).to eq([:ambiguous, nil])
  end

  it "plans Western a number with no format declared" do
    text = %(a\n7\n8\n"1,23,456"\n)
    found = suggest(text, options_class.new(sample_rows: 2))
    expect([found.report[0].door, found.report[0].status]).to eq(%i[i32 resolved])
    same(found.report[0].format, invariant)
    same(found.plan[0].format, invariant)
    batch = one(text, found.plan)
    expect(batch.values(0).first(2)).to eq([7, 8])
    expect(batch.get(0, 2).reason).to eq(:malformed)
  end

  it "doesn't judge a headerless first record as a header" do
    headerless = HyperTabular::Dialect.new(separator: ",", has_header: false)
    text = "\xE9t\xE9,1\nx,2\n".b
    found = suggest(text, options_class::DEFAULT, headerless)
    expect([found.plan.size, found.report[0].status]).to eq([2, :unresolved])
    expect([found.report[0].matched, found.report[0].sampled, found.report[1].door]).to eq([0, 2, :i32])
    Tempfile.create(["bad", ".csv"]) do |file|
      file.binmode
      file.write(text)
      file.close
      expect(reader.suggest(file.path, headerless)).to eq(found)
    end
    # With a header it is structural, and a read says the same.
    expect { suggest(text) }.to raise_error(HyperTabular::TabularError)
    expect { reader.new(text, csv) }.to raise_error(HyperTabular::TabularError)
  end

  it "agrees with a read on a lone zero" do
    { %(a\n"0,125"\n) => invariant, "a\n0.125\n" => comma }.each do |text, format|
      found = suggest(text, options_class.new(format: format))
      expect([found.report[0].door, found.report[0].status]).to eq(%i[text resolved])
      expect(one(text, [column.f64(0, format)]).get(0, 0).reason).to eq(:malformed)
    end
    found = suggest(%(a\n"0,125"\n))
    same(found.report[0].format, comma)
    expect(one(%(a\n"0,125"\n), found.plan).values(0)).to eq([0.125])
    # Beside `1,234`, it's what rules out the comma decimal.
    found = suggest(%(a\n"1,234"\n0.125\n))
    expect([found.report[0].door, found.report[0].status]).to eq(%i[f64 resolved])
    same(found.report[0].format, invariant)
    found = suggest("a\n0.125\n")
    same(found.report[0].format, invariant)
    expect(one("a\n0.125\n", found.plan).values(0)).to eq([0.125])
  end

  it "plans in the comma decimal a column only it reads" do
    text = %(f,i\n"2,5",1.234.567\n"1.234,5",1.000\n"1,234","1,234,567"\n)
    found = suggest(text, options_class.new(sample_rows: 2))
    expect(doors(found)).to eq(%i[f64 i32])
    found.report.each do |said|
      expect([said.status, said.candidates]).to eq([:resolved, []])
      same(said.format, comma)
    end
    same(found.plan[0].format, comma)
    same(found.plan[1].format, comma)
    batch = one(text, found.plan)
    expect(batch.values(0)).to eq([2.5, 1234.5, 1.234])
    expect(batch.values(1).first(2)).to eq([1_234_567, 1000])
    expect(batch.get(1, 2).reason).to eq(:malformed)
  end

  it "reads a question as text, and each answer its own value" do
    text = "a\n1.234\n"
    found = suggest(text)
    expect([found.report[0].status, found.report[0].format, found.plan[0].door]).to eq([:ambiguous, nil, :text])
    read = found.report[0].candidates.map { |format| one(text, [column.f64(0, format)]).values(0)[0] }
    expect(read).to eq([1.234, 1234.0])
  end

  it "plans what a reader takes and a stream reads" do
    text = "id,score\n1,2.5\n2,3.5\n"
    found = suggest(text)
    streamed = reader.new(StringIO.new(text), csv, found.plan)
    expect(streamed.plan).to eq(found.plan)
    expect(streamed.read.values(1)).to eq([2.5, 3.5])
  end

  it "reads an undeclared Indian shape as text, resolved under flags 95 on a number beside it" do
    found = suggest(%(a,b\n"1,23,456",7\n))
    expect([found.report[0].door, found.report[0].status]).to eq(%i[text resolved])
    expect(found.plan[1].format.packed).to eq(invariant.packed)
  end

  it "reports a read's failure past the sample as truncation, not a failure" do
    text = "#{rows_of(3)}\"broken\n"
    found = suggest(text, options_class.new(sample_rows: 2))
    expect([found.truncated, found.report[0].sampled]).to eq([true, 2])
    expect { suggest(text) }.to raise_error(HyperTabular::TabularError)
  end

  context "with a path whose source fails" do
    define_method(:suggest_failing) do |parts|
      source = Class.new do
        define_method(:read) do |_length = nil|
          part = parts.shift
          part.is_a?(Exception) ? raise(part) : part
        end
        define_method(:close) { nil }
      end.new
      allow(File).to receive(:open).and_call_original
      allow(File).to receive(:open).with("/failing.csv", any_args).and_return(source)
      reader.suggest("/failing.csv", csv, options_class.new(sample_rows: 2))
    end

    [Errno::EIO.new, IOError.new("the wire went")].each do |failure|
      it "reports #{failure.class} past a whole sample as truncation" do
        found = suggest_failing(["a\n1\n2\n", failure])
        expect([found.truncated, found.report[0].sampled]).to eq([true, 2])
      end
    end

    it "raises an I/O failure with the sample still short" do
      expect { suggest_failing(["a\n1\n", Errno::EIO.new]) }.to raise_error(Errno::EIO)
    end

    it "raises what isn't an I/O failure, past a whole sample too" do
      expect { suggest_failing(["a\n1\n2\n", ArgumentError.new("custom")]) }.to raise_error(ArgumentError, "custom")
    end
  end

  it "leaves a range's unresolved id column unresolved, and doesn't take a typed second door for a read" do
    ranged = sheet_options.new(range: HyperTabular::Range.parse("B4:C12"))
    report = book("ranges.xlsx").sheet(0, ranged).suggest.report
    said = report.find { |entry| entry.name == "id" }
    expect([said.door, said.status, said.matched, said.sampled]).to eq([:text, :unresolved, 6, 7])
  end
end
