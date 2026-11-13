require "json"

# What the delimited and the workbook replays share: how the corpus spells a plan, a value
# and a failure, and how one cell of a batch is held to it.
module CorpusHelpers
  # The corpus file +name+, found by walking up from this directory.
  def self.corpus(name)
    JSON.parse(File.read(File.join(directory, name), encoding: Encoding::UTF_8))
  end

  # The corpus directory at the repository root.
  def self.directory
    dir = __dir__
    until File.exist?(File.join(dir, "corpus", "delimited.json"))
      raise "corpus directory not found above #{__dir__}" if File.dirname(dir) == dir

      dir = File.dirname(dir)
    end
    File.join(dir, "corpus")
  end

  # Every key the replays read, by where it is: what isn't here fails unknown_keys, so a key added
  # to the corpus can't be passed over. A cell, a problem and a failure compare whole.
  CELL_KEYS = %w[expect value text seconds nanos year month day nanos_of_day magnitude scale negative fault raw
                 raw_hex detail origin keyword adjusted enum_index bits].freeze
  PROBLEM_KEYS = %w[row line plan_column source_column reason fault detail origin expected found keyword].freeze
  FAILURE_KEYS = %w[kind record line byte expected found].freeze
  PLAN_KEYS = %w[door ordinal precision order epoch format tokens options constraints].freeze
  # A plan entry's constraints, by the OpenAPI names HyperCast's corpus gives them, as
  # HyperCast::Constraints.new takes them.
  CONSTRAINT_KEYS = { "minimum" => :minimum, "exclusiveMinimum" => :exclusive_minimum, "maximum" => :maximum,
                      "exclusiveMaximum" => :exclusive_maximum, "multipleOf" => :multiple_of,
                      "minLength" => :min_length, "maxLength" => :max_length, "enum" => :enum, "trim" => :trim,
                      "caseSensitive" => :case_sensitive, "rounding" => :rounding }.freeze
  FORMAT_KEYS = %w[decimal_sep group_sep flags currency].freeze
  DATE_OPTION_KEYS = %w[flags pivot months].freeze
  DELIMITED_KEYS = %w[name input input_hex dialect plan header rows problems failure suggest].freeze
  DIALECT_KEYS = %w[separator quoting has_header skip_blank_lines skip header_row short_rows long_rows tokens
                    encoding].freeze
  SUGGEST_KEYS = %w[columns truncated sample_rows format].freeze
  SUGGESTED_KEYS = %w[door param status matched sampled].freeze
  # note is the one no replay reads: a sentence for whoever reads the file.
  WORKBOOK_KEYS = %w[name file format epoch sheets sheet options plan header numbers rows problems failure note
                     suggest].freeze
  SHEET_OPTION_KEYS = %w[has_header skip_empty_rows range skip header_row tokens].freeze
  SHEET_KEYS = %w[name hidden].freeze

  # What the corpus calls an encoding, as a Dialect declares it.
  ENCODINGS = { "windows-1252" => :windows_1252, "latin-1" => :latin1, "utf-16le" => :utf16le }.freeze
  DATE_FLAGS = { "compact_date" => HyperCast::DateOptions::COMPACT_DATE,
                 "space_separator" => HyperCast::DateOptions::SPACE_SEPARATOR,
                 "hour_offset" => HyperCast::DateOptions::HOUR_OFFSET }.freeze
  NUMERIC_DOORS = HyperTabular::Column::NUMERIC

  # Every key of a case no replay here reads, as a path.
  def self.unknown_keys(vector, workbook:)
    name = vector.fetch("name", "a case with no name")
    unknown = []
    check = lambda do |value, path, known|
      value.each_key { |key| unknown << "#{path}.#{key}" unless known.include?(key) } if value.is_a?(Hash)
    end
    check.call(vector, name, workbook ? WORKBOOK_KEYS : DELIMITED_KEYS)
    if workbook
      check.call(vector["options"], "#{name}.options", SHEET_OPTION_KEYS)
      vector.fetch("sheets", []).each { |sheet| check.call(sheet, "#{name}.sheets[]", SHEET_KEYS) }
    else
      check.call(vector["dialect"], "#{name}.dialect", DIALECT_KEYS)
    end
    vector.fetch("plan", []).each do |column|
      check.call(column, "#{name}.plan[]", PLAN_KEYS)
      check.call(column["format"], "#{name}.plan[].format", FORMAT_KEYS)
      check.call(column["options"], "#{name}.plan[].options", DATE_OPTION_KEYS)
      check.call(column["constraints"], "#{name}.plan[].constraints", CONSTRAINT_KEYS.keys)
    end
    vector.fetch("rows", []).flatten(1).each { |cell| check.call(cell, "#{name}.rows[][]", CELL_KEYS) }
    vector.fetch("problems", []).each { |problem| check.call(problem, "#{name}.problems[]", PROBLEM_KEYS) }
    check.call(vector["failure"], "#{name}.failure", FAILURE_KEYS)
    if (suggest = vector["suggest"])
      check.call(suggest, "#{name}.suggest", SUGGEST_KEYS)
      check.call(suggest["format"], "#{name}.suggest.format", FORMAT_KEYS)
      suggest["columns"].each { |column| check.call(column, "#{name}.suggest.columns[]", SUGGESTED_KEYS) }
    end
    unknown
  end

  # Every object in +value+, the first of each list only, as the path to it.
  def self.key_places(value, path = [])
    case value
    when Hash then [path] + value.flat_map { |key, inner| key_places(inner, path + [key]) }
    when Array then value.empty? ? [] : key_places(value.first, path + [0])
    else []
    end
  end

  # Nothing in the corpus is passed over, and the check notices a key added anywhere: a failure
  # message, or nil.
  def self.keys_problem(vectors, workbook:)
    passed_over = vectors.flat_map { |vector| unknown_keys(vector, workbook: workbook) }
    return passed_over.first(5).inspect unless passed_over.empty?

    vectors.each do |vector|
      key_places(vector).each do |place|
        edited = JSON.parse(JSON.generate(vector))
        (place.empty? ? edited : edited.dig(*place))["junk"] = true
        unknown = unknown_keys(edited, workbook: workbook)
        return [place, unknown].inspect unless unknown.size == 1 && unknown.first.end_with?(".junk")
      end
    end
    nil
  end

  # The bytes of a case's input.
  def input_of(vector)
    vector.key?("input_hex") ? [vector["input_hex"]].pack("H*") : vector["input"]
  end

  # A case's dialect, as the corpus declares it.
  def dialect_of(settings)
    declared = settings.to_h { |key, value| [key.to_sym, value] }
    declared[:short_rows] = declared[:short_rows].to_sym if declared.key?(:short_rows)
    declared[:long_rows] = declared[:long_rows].to_sym if declared.key?(:long_rows)
    declared[:encoding] = ENCODINGS.fetch(declared[:encoding]) if declared.key?(:encoding)
    HyperTabular::Dialect.new(**declared)
  end

  # A case's sheet options.
  def options_of(settings, batch_rows, notes: true)
    range = settings["range"]
    HyperTabular::SheetOptions.new(
      has_header: settings["has_header"], skip_empty_rows: settings["skip_empty_rows"], batch_rows: batch_rows,
      skip: settings.fetch("skip", 0), header_row: settings.fetch("header_row", 0),
      range: range && HyperTabular::Range.parse(range), tokens: settings.fetch("tokens", []), notes: notes
    )
  end

  # The format an entry names, its flags split as HyperCast's constructor takes them: the style
  # flags, and the grouping policy above them. An unknown policy fails the replay.
  def format_of(entry)
    spec = entry["format"]
    return HyperCast::NumFormat::INVARIANT unless spec

    policy = spec["flags"] & ~0xFF
    grouping = HyperCast::GROUPING_POLICIES.key(policy)
    raise "the corpus declares a grouping policy this replay does not know: #{policy}" if grouping.nil?

    HyperCast::NumFormat.new(decimal_sep: spec["decimal_sep"], group_sep: spec["group_sep"],
                             flags: spec["flags"] & 0xFF, currency: spec.fetch("currency", ""), grouping: grouping)
  end

  # A plan entry's tokens and date options.
  def declared_options(column, entry)
    column = column.with_tokens(entry["tokens"]) if entry.key?("tokens")
    options = entry["options"]
    return column if options.nil?

    flags = options["flags"].sum { |name| DATE_FLAGS.fetch(name) }
    months = options.key?("months") ? HyperCast::MonthNames.new(entries: options["months"].map(&:dup)) : nil
    column.with_date_options(HyperCast::DateOptions.new(flags: flags, pivot: options.fetch("pivot", 0)), months)
  end

  # A plan entry's constraints as HyperCast's own declaration for the door it names, or nil.
  def constraints_of(door, entry)
    declared = entry["constraints"]
    return nil if declared.nil?

    keywords = declared.to_h { |key, value| [CONSTRAINT_KEYS.fetch(key), value] }
    keywords[:rounding] = keywords[:rounding].to_sym if keywords.key?(:rounding)
    HyperCast::Constraints.new(door: door, **keywords)
  end

  def column_of(entry)
    ordinal = entry["ordinal"]
    door = entry["door"].to_sym
    constraints = constraints_of(door, entry)
    built = case door
            when :unix then HyperTabular::Column.unix(ordinal, HyperCast::UNIX_PRECISIONS.key(entry["precision"]))
            when :excel_serial
              HyperTabular::Column.excel_serial(ordinal, HyperCast::EXCEL_EPOCHS.key(entry["epoch"]))
            when :date_ordered, :datetime
              HyperTabular::Column.public_send(door, ordinal, HyperCast::DATE_ORDERS.key(entry["order"]))
            when *NUMERIC_DOORS
              # An entry with no format builds a column given none, so that the vector says what a
              # column with no format reads.
              if entry.key?("format")
                HyperTabular::Column.public_send(door, ordinal, format_of(entry), constraints: constraints)
              else
                HyperTabular::Column.public_send(door, ordinal, constraints: constraints)
              end
            when :text then HyperTabular::Column.text(ordinal, constraints: constraints)
            # A Column refuses a format on a door that reads none, as it has since 1.0; a suggested
            # text column's format in the corpus says nothing a read could show.
            else HyperTabular::Column.public_send(door, ordinal)
            end
    declared_options(built, entry)
  end

  # What the corpus says a cell that cast holds, as the Ruby value this binding owes for
  # the door: the type HyperCast's gem returns from the same one.
  def expected_value(door, cell)
    case door
    when :f32 then [cell["value"]].pack("e").unpack1("e")
    when :f64 then cell["value"].to_f
    when :decimal
      HyperCast::Decimal.new(magnitude: Integer(cell["magnitude"]), scale: cell["scale"], negative: cell["negative"])
    when :uuid then cell["value"].unpack("a8a4a4a4a12").join("-")
    when :timestamp, :unix, :excel_serial then Time.at(cell["seconds"], cell["nanos"], :nanosecond, in: "UTC")
    when :date, :date_ordered then Date.new(cell["year"], cell["month"], cell["day"])
    when :datetime
      second_of_day, nanos = cell["nanos_of_day"].divmod(1_000_000_000)
      DateTime.new(cell["year"], cell["month"], cell["day"], second_of_day / 3600, second_of_day / 60 % 60,
                   second_of_day % 60 + Rational(nanos, 1_000_000_000))
    when :time then cell["nanos"]
    when :duration then Rational(cell["seconds"] * 1_000_000_000 + cell["nanos"], 1_000_000_000)
    when :text then cell["text"]
    else cell["value"]
    end
  end

  # Holds one cell of a batch to what the corpus says of it: the verdict, and where it came from. A
  # cell of a constrained column is held through the checked getter (assert_checked).
  def assert_cell(label, batch, column, row, expected, notes: true)
    door = batch.columns[column].door
    raw = batch.raw(column, row)
    expect(raw.encoding).to eq(Encoding::UTF_8), label
    origin = notes ? expected.fetch("origin", "cell") : "cell"
    expect(batch.origin(column, row).to_s).to eq(origin), "#{label}: origin"
    return assert_checked(label, batch, column, row, expected, notes: notes) if batch.columns[column].checked?

    # The union consumption idiom in action — pattern matching over the two case types.
    case batch.get(column, row)
    in HyperCast::Success(value:)
      expect(expected["expect"]).to eq("ok"), "#{label}: unexpectedly cast to #{value.inspect}"
      wanted = expected_value(door, expected)
      expect(value).to eq(wanted), "#{label}: #{value.inspect}, want #{wanted.inspect}"
      expect(value.class).to eq(wanted.class), label
      expect(value.to_s).to eq(expected["value"]), label if door == :decimal
      expect([value.utc?, value.nsec]).to eq([true, expected["nanos"]]), label if value.is_a?(Time)
      expect([value, value.encoding]).to eq([raw, Encoding::UTF_8]), label if door == :text
      expect(batch.values(column)[row]).to equal(value), label
    in HyperCast::Fault => fault
      assert_fault(label, batch, column, row, expected, fault, notes: notes)
    end
  end

  # Holds a Fault to what the corpus says of the cell: its reason, detail and span, and no value.
  def assert_fault(label, batch, column, row, expected, fault, notes: true)
    raw = batch.raw(column, row)
    reason, offset, length, detail = fault.reason, fault.offset, fault.length, fault.detail
    expect(reason.to_s).to eq(expected["expect"]), "#{label}: faulted #{reason} on #{raw.inspect}"
    expect(batch.values(column)[row]).to be_nil, label
    expect(detail&.name&.to_s).to eq(notes ? expected["detail"] : nil), "#{label}: detail #{detail.inspect}"
    expect(raw).to eq(""), label if expected["expect"] == "empty" && !expected.key?("origin")
    if (span = expected["fault"])
      # The cell's own text is still to hand, for the diagnostic a fault deserves.
      if expected.key?("raw_hex")
        expect(raw.b).to eq([expected["raw_hex"]].pack("H*")), label
      else
        expect(raw).to eq(expected["raw"]), label
      end
      expect([offset, length])
        .to eq([raw.byteslice(0, span[0]).length, raw.byteslice(span[0], span[1]).length]), label
    else
      expect([offset, length]).to eq([0, 0]), label
    end
  end

  # Holds one cell of a constrained column to what the corpus says of it, through the checked
  # getter: what passed, a violation of a constraint the column declares, or the cell's fault. The
  # plain getter refuses the column, and #values holds a violation's value as cast.
  def assert_checked(label, batch, column, row, expected, notes: true)
    plan = batch.columns[column]
    raw = batch.raw(column, row)
    expect { batch.get(column, row) }.to raise_error(TypeError, /get_checked/), label
    verdict = batch.get_checked(column, row)
    case verdict
    in HyperCast::Checked(value:, adjusted:, enum_index:)
      expect(expected["expect"]).to eq("ok"), "#{label}: unexpectedly passed with #{verdict.inspect}"
      expect([adjusted, enum_index])
        .to eq([expected["adjusted"], expected["enum_index"]]), "#{label}: #{verdict.inspect}"
      if enum_index && plan.door == :text
        # The entry as declared: the very String the declaration holds.
        expect(value).to equal(plan.constraints.enum[enum_index]), label
      else
        expect(batch.values(column)[row]).to eql(value), label
      end
      assert_checked_value(label, plan.door, value, expected)
    in HyperCast::Violation(keyword:, value:, offset:, length:)
      expect(expected["expect"]).to eq("violation"), "#{label}: unexpectedly broke #{verdict.inspect}"
      expect(keyword.name).to eq(expected["keyword"]), "#{label}: #{verdict.inspect}"
      expect(raw).to eq(expected["raw"]), label
      span = expected["fault"]
      expect([offset, length])
        .to eq([raw.byteslice(0, span[0]).length, raw.byteslice(span[0], span[1]).length]), "#{label}: span"
      expect(batch.values(column)[row]).to eql(value), "#{label}: the value as cast"
      assert_checked_value(label, plan.door, value, expected)
    in HyperCast::Fault => fault
      assert_fault(label, batch, column, row, expected, fault, notes: notes)
    end
  end

  # A checked cell's value, passed or as cast: a float to its exact bits, anything else as
  # expected_value says, of the same class.
  def assert_checked_value(label, door, value, expected)
    if %i[f32 f64].include?(door)
      bits = "0x#{[value].pack(door == :f32 ? 'g' : 'G').unpack1('H*')}"
      expect([value.class, bits]).to eq([Float, expected["bits"]]), "#{label}: #{value.inspect}"
    else
      wanted = expected_value(door, expected)
      expect([value, value.class]).to eq([wanted, wanted.class]), "#{label}: #{value.inspect}, want #{wanted.inspect}"
    end
  end

  def assert_failure(label, actual, vector)
    expected = vector["failure"]
    if expected.nil?
      expect(actual).to be_nil, "#{label}: #{actual&.message}"
      return
    end
    expect(actual).to be_a(HyperTabular::TabularError), "#{label}: no failure"
    # The whole failure, an absent expected and found being 0.
    expect([actual.kind.to_s, actual.record, actual.line, actual.byte, actual.expected, actual.found])
      .to eq([*expected.values_at("kind", "record", "line", "byte"), expected.fetch("expected", 0),
              expected.fetch("found", 0)]), label
  end

  # An entry of a batch's faults list in the corpus's shape, its row counted across the read, its
  # span in the characters a Problem counts.
  def problem_json(batch, problem, base)
    entry = { "row" => base + problem.row, "line" => problem.line }
    expect(problem.line).to eq(batch.line(problem.row))
    if problem.plan_column.nil?
      expect([problem.source_column, problem.reason, problem.offset, problem.length, problem.keyword])
        .to eq([nil, nil, 0, 0, nil])
      entry["expected"] = problem.expected
      entry["found"] = problem.found
    elsif problem.keyword
      # A violation: the cell cast, so there's no reason, and the span is the judged text's.
      expect([problem.reason, problem.expected, problem.found]).to eq([nil, 0, 0])
      entry["plan_column"] = problem.plan_column
      entry["source_column"] = problem.source_column
      entry["reason"] = "violation"
      entry["keyword"] = problem.keyword.name
      entry["fault"] = [problem.offset, problem.length]
      verdict = batch.get_checked(problem.plan_column, problem.row)
      expect([verdict.keyword, verdict.offset, verdict.length]).to eq([problem.keyword, problem.offset, problem.length])
    else
      expect([problem.expected, problem.found]).to eq([0, 0])
      entry["plan_column"] = problem.plan_column
      entry["source_column"] = problem.source_column
      entry["reason"] = problem.reason.to_s
      if problem.reason == :empty
        expect([problem.offset, problem.length]).to eq([0, 0])
      else
        entry["fault"] = [problem.offset, problem.length]
        verdict = batch.send(:verdict_of, problem.plan_column, problem.row)
        expect([verdict.offset, verdict.length]).to eq([problem.offset, problem.length])
      end
    end
    entry["detail"] = problem.detail.name.to_s unless problem.detail.nil?
    entry["origin"] = problem.origin.to_s unless problem.origin == :cell
    entry
  end

  # The case's problems as a read should say them: the corpus counts a span in bytes, and a Problem
  # in characters, through the text of the cell the corpus has at that row and plan column.
  def problems_of(vector, notes: true)
    vector["problems"].map do |problem|
      next problem unless problem.key?("fault")

      cell = vector["rows"].fetch(problem["row"]).fetch(problem["plan_column"])
      raw = cell.key?("raw_hex") ? [cell["raw_hex"]].pack("H*").force_encoding(Encoding::UTF_8) : cell.fetch("raw")
      offset, length = problem["fault"]
      problem.merge("fault" => [raw.byteslice(0, offset).length, raw.byteslice(offset, length).length])
    end.then { |listed| notes ? listed : problems_without_notes(listed) }
  end

  # What a read with notes off lists of +problems+: no formula with no cached value, and no detail
  # or origin on the rest.
  def problems_without_notes(problems)
    problems.reject { |problem| problem["origin"] == "uncached" }
            .map { |problem| problem.except("detail", "origin") }
  end

  # Holds the batch's row view to the batch: every row, in order, each member the batch's own call
  # with the row's index.
  def assert_rows(label, plan, batch)
    rows = batch.to_a
    expect(rows.map(&:index)).to eq((0...batch.rows).to_a), label
    rows.each do |row|
      expect(row.line).to eq(batch.line(row.index)), label
      expect(row.to_a).to eq(plan.each_index.map { |column| batch.send(:verdict_of, column, row.index) }), label
      plan.each_index do |column|
        if plan[column].checked?
          expect(row.get_checked(column)).to eq(batch.get_checked(column, row.index)), label
          expect { row.get(column) }.to raise_error(TypeError, /get_checked/), label
        else
          expect(row.get(column)).to eq(batch.get(column, row.index)), label
        end
        expect(row.raw(column)).to eq(batch.raw(column, row.index)), label
        expect(row.origin(column)).to eq(batch.origin(column, row.index)), label
      end
    end
  end

  # A report in the corpus's shape: an ambiguous column's param is the set of its candidates, and a
  # resolved number's is left at 0, since a report names the format the plan uses.
  def suggestion_json(found)
    found.report.map do |column|
      param = 0
      if !column.candidates.empty?
        column.candidates.each do |candidate|
          param |= if candidate.is_a?(Symbol)
                     1 << HyperCast::DATE_ORDERS.fetch(candidate)
                   else
                     candidate.decimal_sep == "." ? 2 : 4
                   end
        end
      elsif %i[date_ordered datetime].include?(column.door)
        param = HyperCast::DATE_ORDERS.fetch(column.declared)
      end
      { "door" => column.door.to_s, "param" => param, "status" => column.status.to_s,
        "matched" => column.matched, "sampled" => column.sampled }
    end
  end

  # What a report should say of the corpus's columns: see suggestion_json.
  def reported(columns)
    columns.map do |column|
      dated = %w[date_ordered datetime].include?(column["door"])
      column["status"] == "resolved" && !dated ? column.merge("param" => 0) : column
    end
  end

  # Each resolved number is planned and reported in the format the corpus's param says, or the one
  # declared; anything else reports none.
  def assert_notation(label, found, columns, declared)
    found.report.zip(found.plan, columns).each do |report, planned, want|
      if report.status == :resolved && NUMERIC_DOORS.include?(report.door)
        comma = HyperCast::NumFormat.new(decimal_sep: ",", group_sep: ".", flags: HyperCast::ALL_STYLES)
        wanted = declared || (want["param"] == 4 ? comma : HyperCast::NumFormat::INVARIANT)
        expect(report.format.packed).to eq(wanted.packed), label
        expect(planned.format.packed).to eq(wanted.packed), label
      else
        expect(report.format).to be_nil, label
      end
    end
  end
end
