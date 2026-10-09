module HyperTabular
  # One row of a Batch: what Batch#each, DelimitedReader#each and Sheet#each yield, for a
  # caller that thinks in rows — "for each record, build an object from these columns" —
  # rather than in columns. A view of (batch, index): every member answers what the batch's
  # own accessor answers for that row, nothing is copied, and since a batch owns what it
  # shows, a row stays good after the reader has moved on.
  #
  #   reader.each do |row|
  #     case row.get(0)
  #     in HyperCast::Success(value: id) then orders << Order.new(id, row.value(1), row.line)
  #     in HyperCast::Fault(reason:) then warn "line #{row.line}: #{reason} in #{row.raw(0).inspect}"
  #     end
  #   end
  #
  # It deconstructs as its verdicts, one per plan column, so a whole row can be matched at
  # once: `in [HyperCast::Success(value: id), HyperCast::Success(value: name)]`.
  class Row
    # The batch the row is in.
    attr_reader :batch

    # The row's 0-based index within its batch.
    attr_reader :index

    # Made by Batch#each.
    def initialize(batch, index)
      @batch = batch
      @index = index
      freeze
    end

    # Where the row came from — Batch#line: for delimited text the 1-based line its record
    # starts on, for a sheet its 1-based row number.
    def line
      @batch.line(@index)
    end

    # The cell in plan column +column+ as HyperCast judged it — Batch#get. IndexError for a
    # column outside the plan.
    def get(column)
      @batch.get(column, @index)
    end
    alias [] get
    alias verdict get

    # The cell's value, or nil where it did not cast — Batch#values' entry for this row.
    # IndexError for a column outside the plan.
    def value(column)
      @batch.values(column)[@index]
    end

    # The text the cell was cast from, whatever its door and verdict — Batch#raw.
    def raw(column)
      @batch.raw(column, @index)
    end

    # How many cells the row has: one per plan column.
    def size
      @batch.columns.size
    end
    alias length size

    # Every cell's verdict, in plan order, as a frozen Array — the Array
    # DelimitedReader#each_row yields for the same row.
    def to_a
      Array.new(size) { |column| get(column) }.freeze
    end
    alias deconstruct to_a

    # The row in a line — not its cells.
    def inspect
      "#<#{self.class.name} index=#{@index} line=#{line}>"
    end
  end
end
