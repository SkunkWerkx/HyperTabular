# A column that declares HyperCast's constraints: what the corpus does not reach of the binding's
# surface. A Ruby column can be handed any declaration, so the pairing with its door is checked
# when the column is made and again when the plan is bound.

require "spec_helper"
require_relative "corpus_helpers"

RSpec.describe "a column that declares constraints" do
  headerless = HyperTabular::Dialect::CSV.with(has_header: false)

  define_method(:read) do |csv, *plan|
    batch = HyperTabular::DelimitedReader.new(csv, headerless, plan).read
    expect(batch).not_to be_nil
    batch
  end

  def at_most(maximum)
    HyperCast::Constraints.new(door: :i32, maximum: maximum)
  end

  # A Column built without Column#initialize, as a caller could, or a Marshal payload might.
  def smuggled(door, constraints)
    column = HyperTabular::Column.allocate
    Data.instance_method(:initialize).bind_call(column, ordinal: 0, door: door, declared: nil, format: nil, tokens: nil,
                                                        date_options: nil, months: nil, constraints: constraints)
    column
  end

  it "reads a violation, keeping the value as cast" do
    batch = read("11\n7\nx\n", HyperTabular::Column.i32(0, constraints: at_most(10)))
    expect(batch.columns[0].constraints).to eq(at_most(10))

    expect(batch.get_checked(0, 0)).to eq(HyperCast::Violation.new(keyword: HyperCast::Keyword::MAXIMUM, value: 11,
                                                                   offset: 0, length: 2))
    expect(batch.values(0)).to eq([11, 7, nil]), "the value as cast, kept"
    expect(batch.get_checked(0, 1)).to eq(HyperCast::Checked.new(value: 7, adjusted: false, enum_index: nil))
    expect(batch.get_checked(0, 2)).to have_attributes(class: HyperCast::Fault, reason: :malformed)
    expect(batch.row(1).get_checked(0)).to eq(batch.get_checked(0, 1))
  end

  it "flags a value its multiple_of rounding adjusted" do
    declared = HyperCast::Constraints.new(door: :decimal, multiple_of: "0.5", rounding: :floor)
    batch = read("1.3\n1.5\n", HyperTabular::Column.decimal(0, constraints: declared))
    one = HyperCast::Decimal.new(magnitude: 1, scale: 0, negative: false)
    expect(batch.get_checked(0, 0)).to eq(HyperCast::Checked.new(value: one, adjusted: true, enum_index: nil))
    one_and_a_half = HyperCast::Decimal.new(magnitude: 15, scale: 1, negative: false)
    expect(batch.get_checked(0, 1)).to eq(HyperCast::Checked.new(value: one_and_a_half, adjusted: false,
                                                                 enum_index: nil))
  end

  it "delivers a text enum match as the entry declared" do
    declared = HyperCast::Constraints.new(door: :text, enum: %w[Female Male])
    batch = read("MALE\nother\n", HyperTabular::Column.text(0, constraints: declared))
    passed = batch.get_checked(0, 0)
    expect(passed).to eq(HyperCast::Checked.new(value: "Male", adjusted: false, enum_index: 1))
    expect(passed.value).to equal(declared.enum[1]), "the declaration's own String"
    expect(batch.get_checked(0, 1)).to eq(HyperCast::Violation.new(keyword: HyperCast::Keyword::ENUM, value: "other",
                                                                   offset: 0, length: 5))
  end

  it "reads an unconstrained column through the checked getter, faults and all" do
    batch = read("5\n\"\"\nx\n", HyperTabular::Column.i32(0))
    expect(batch.columns[0].constraints).to be_nil
    expected = [HyperCast::Checked.new(value: 5, adjusted: false, enum_index: nil), batch.get(0, 1), batch.get(0, 2)]
    expect(3.times.map { |row| batch.get_checked(0, row) }).to eq(expected)
    expect(expected.drop(1).map(&:reason)).to eq(%i[empty malformed])

    # No stale metadata reaches it: a column that declares nothing has no checks read for it, so
    # checks a batch was handed for it by mistake change nothing.
    batch.instance_variable_set(:@checks, ["\x05\0\0\0\x01\0\0\0\x00\0\0\0".b * 3])
    expect(3.times.map { |row| batch.get_checked(0, row) }).to eq(expected)
  end

  it "refuses the plain getters a constrained column, which would read a violation as passed" do
    batch = read("7,a\n", HyperTabular::Column.i32(0, constraints: at_most(10)),
                 HyperTabular::Column.text(1, constraints: HyperCast::Constraints.new(door: :text, max_length: 3)),
                 HyperTabular::Column.i32(0))
    refusals = {
      "get" => -> { batch.get(0, 0) }, "verdicts" => -> { batch.verdicts(1) },
      "row.get" => -> { batch.row(0).get(0) }, "row[]" => -> { batch.row(0)[1] }
    }
    refusals.each do |name, read|
      expect(&read).to raise_error(TypeError, "column #{name == 'row[]' || name == 'verdicts' ? 1 : 0} declares " \
                                              "constraints: read it with get_checked"), name
    end
    # The control: the checked getter reads them, the unconstrained column reads as ever, and a row
    # is each column as its own getter reads it.
    expect(batch.get_checked(0, 0)).to eq(HyperCast::Checked.new(value: 7, adjusted: false, enum_index: nil))
    expect(batch.get_checked(1, 0)).to eq(HyperCast::Checked.new(value: "a", adjusted: false, enum_index: nil))
    expect(batch.get(2, 0)).to eq(HyperCast::Success.new(value: 7))
    expect(batch.values(0)).to eq([7])
    expect(batch.row(0).to_a).to eq([batch.get_checked(0, 0), batch.get_checked(1, 0), batch.get(2, 0)])
  end

  it "reads a whole column as checked verdicts, a constrained one or not" do
    batch = read("11\n7\nx\n", HyperTabular::Column.i32(0, constraints: at_most(10)), HyperTabular::Column.i32(0))
    [0, 1].each do |column|
      verdicts = batch.checked_verdicts(column)
      expect(verdicts).to eq(3.times.map { |row| batch.get_checked(column, row) }), "column #{column}"
      expect(verdicts).to be_frozen
    end
    expect(batch.checked_verdicts(0).map(&:class)).to eq([HyperCast::Violation, HyperCast::Checked, HyperCast::Fault])
    expect(batch.checked_verdicts(1).first).to eq(HyperCast::Checked.new(value: 11, adjusted: false, enum_index: nil))
    expect { batch.checked_verdicts(2) }.to raise_error(IndexError)
  end

  it "yields each row's checked verdicts from each_row" do
    plan = [HyperTabular::Column.i32(0, constraints: at_most(10))]
    reader = HyperTabular::DelimitedReader.new("11\n7\n", headerless, plan)
    expect(reader.each_row.to_a).to eq([[HyperCast::Violation.new(keyword: HyperCast::Keyword::MAXIMUM, value: 11,
                                                                  offset: 0, length: 2)],
                                        [HyperCast::Checked.new(value: 7, adjusted: false, enum_index: nil)]])
  end

  it "refuses constraints declared for another door, when the column is made and when it is bound" do
    expect { HyperTabular::Column.u8(0, constraints: at_most(10)) }
      .to raise_error(ArgumentError, "Constraints for :i32 were given to a :u8 column")
    expect { HyperTabular::Column.i32(0, constraints: at_most(10)).with(door: :i64) }
      .to raise_error(ArgumentError, "Constraints for :i32 were given to a :i64 column")
    expect { HyperTabular::Column.i32(0, constraints: { maximum: 10 }) }
      .to raise_error(ArgumentError, /constraints must be a HyperCast::Constraints/)
    expect { HyperTabular::Column.i32(0, constraints: HyperCast::Constraints.allocate) }
      .to raise_error(ArgumentError, /was never built/)

    # A column that got past Column#initialize is refused when its plan is bound, on a reader made
    # with the plan and on one bound to it after its header.
    expect { HyperTabular::DelimitedReader.new("1\n", headerless, [smuggled(:i64, at_most(10))]) }
      .to raise_error(ArgumentError,
                      "plan column 0 cannot be honoured: Constraints for :i32 were given to a :i64 column")
    reader = HyperTabular::DelimitedReader.new("1\n", headerless)
    expect { reader.bind([smuggled(:i64, at_most(10))]) }.to raise_error(ArgumentError, /were given to a :i64 column/)
    # A door that takes no constraints has none to take.
    expect { HyperTabular::DelimitedReader.new("1\n", headerless, [smuggled(:uuid, HyperCast::Constraints.new(door: :text))]) }
      .to raise_error(ArgumentError, /Constraints for :text were given to a :uuid column/)
    # The control: the same columns, rightly paired, are bound.
    expect(HyperTabular::DelimitedReader.new("1\n", headerless, [smuggled(:i32, at_most(10))]).read.get_checked(0, 0))
      .to eq(HyperCast::Checked.new(value: 1, adjusted: false, enum_index: nil))
  end

  it "lists a violation in faults with its keyword" do
    listed = read("11\nx\n", HyperTabular::Column.i32(0, constraints: at_most(10))).faults
    expect(listed.map { |problem| [problem.keyword, problem.reason, problem.offset, problem.length] })
      .to eq([[HyperCast::Keyword::MAXIMUM, nil, 0, 2], [nil, :malformed, 0, 1]])
  end

  it "sizes the checks like the verdicts" do
    batch = read("11\n" * 37, HyperTabular::Column.i32(0, constraints: at_most(10)))
    expect(batch.rows).to eq(37)
    expect(batch.get_checked(0, 36)).to eq(HyperCast::Violation.new(keyword: HyperCast::Keyword::MAXIMUM, value: 11,
                                                                    offset: 0, length: 2))
    expect(batch.faults.map(&:keyword)).to eq([HyperCast::Keyword::MAXIMUM] * 37)
  end

  it "reads a constrained sheet column" do
    path = File.join(CorpusHelpers.directory, "workbook", "constraints.xlsx")
    sheet = HyperTabular::Workbook.open(path).sheet(0, HyperTabular::SheetOptions.new(has_header: true),
                                                    [HyperTabular::Column.i32(0, constraints: at_most(10))])
    verdicts = sheet.map { |row| row.get_checked(0) }
    expect(verdicts.first).to eq(HyperCast::Checked.new(value: 0, adjusted: false, enum_index: nil))
    expect(verdicts).to include(HyperCast::Violation.new(keyword: HyperCast::Keyword::MAXIMUM, value: 42, offset: 0,
                                                         length: 0))
  end

  it "is equal to the same column, and says its constraints" do
    column = HyperTabular::Column.i32(0, constraints: at_most(10))
    expect(column).to eq(HyperTabular::Column.i32(0, constraints: at_most(10)))
    expect(column).not_to eq(HyperTabular::Column.i32(0))
    expect(column).not_to eq(HyperTabular::Column.i32(0, constraints: at_most(11)))
    expect(column.inspect).to include("constraints=#<data HyperCast::Constraints door=:i32")
  end
end
