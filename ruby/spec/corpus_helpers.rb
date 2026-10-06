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

  def format_of(entry)
    spec = entry["format"]
    return HyperCast::NumFormat::INVARIANT unless spec

    HyperCast::NumFormat.new(decimal_sep: spec["decimal_sep"], group_sep: spec["group_sep"],
                             flags: spec["flags"], currency: spec.fetch("currency", ""))
  end

  def column_of(entry)
    ordinal = entry["ordinal"]
    door = entry["door"].to_sym
    case door
    when :unix then HyperTabular::Column.unix(ordinal, HyperCast::UNIX_PRECISIONS.key(entry["precision"]))
    when :excel_serial then HyperTabular::Column.excel_serial(ordinal, HyperCast::EXCEL_EPOCHS.key(entry["epoch"]))
    when :date_ordered, :datetime
      HyperTabular::Column.public_send(door, ordinal, HyperCast::DATE_ORDERS.key(entry["order"]))
    when *HyperTabular::Column::NUMERIC then HyperTabular::Column.public_send(door, ordinal, format_of(entry))
    else HyperTabular::Column.public_send(door, ordinal)
    end
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

  # Holds one cell of a batch to what the corpus says of it.
  def assert_cell(label, batch, column, row, expected)
    door = batch.columns[column].door
    raw = batch.raw(column, row)
    expect(raw.encoding).to eq(Encoding::UTF_8), label
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
    in HyperCast::Fault(reason:, offset:, length:)
      expect(reason.to_s).to eq(expected["expect"]), "#{label}: faulted #{reason} on #{raw.inspect}"
      expect(batch.values(column)[row]).to be_nil, label
      if (span = expected["fault"])
        # The cell's own text is still to hand, for the diagnostic a fault deserves.
        expect(raw).to eq(expected["raw"]), label
        expect([offset, length])
          .to eq([raw.byteslice(0, span[0]).length, raw.byteslice(span[0], span[1]).length]), label
      else
        expect([offset, length]).to eq([0, 0]), label
      end
    end
  end

  def assert_failure(label, actual, vector)
    expected = vector["failure"]
    if expected.nil?
      expect(actual).to be_nil, "#{label}: #{actual&.message}"
      return
    end
    expect(actual).to be_a(HyperTabular::TabularError), "#{label}: no failure"
    expect([actual.kind.to_s, actual.record, actual.line, actual.byte])
      .to eq(expected.values_at("kind", "record", "line", "byte")), label
    return unless expected.key?("expected")

    expect([actual.expected, actual.found]).to eq(expected.values_at("expected", "found")), label
  end
end
