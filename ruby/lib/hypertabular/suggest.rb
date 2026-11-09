module HyperTabular
  # How a sample is taken and read: DelimitedReader.suggest, Sheet#suggest. Declaring the locale
  # here is what settles what a sample can't.
  #
  # +sample_rows+ is the most rows the sample holds, 0 taken as 1; it stops at 4096 rows of delimited
  # text, and at a sheet's own batch size, read in as many batches as the core says fit: a sample is
  # never refused for the size of its batch. +format+ reads every column's numbers
  # in this HyperCast::NumFormat, which settles what "," and "." mean in them. +column_formats+ is
  # per source column, ahead of +format+, the last given winning: a Hash or an Array of pairs, and
  # every format given is judged first of anything, whether it's replaced or the sample ever reaches
  # its ordinal. +date_order+ reads every column's dates in this order where the sample leaves the
  # order open, as 01/02/2026 does, and +column_date_orders+ is per source column, ahead of it.
  SuggestOptions = Data.define(:sample_rows, :format, :column_formats, :date_order, :column_date_orders) do
    # ArgumentError for a negative count of rows or an entry that isn't [ordinal, value]; KeyError
    # for an undeclared date order. A format the core can't honour is the suggest's to refuse.
    def initialize(sample_rows: 1000, format: nil, column_formats: [], date_order: nil, column_date_orders: [])
      raise ArgumentError, "sample_rows must be an Integer at least 0; got #{sample_rows.inspect}" unless
        sample_rows.is_a?(Integer) && sample_rows >= 0

      HyperCast::DATE_ORDERS.fetch(date_order) unless date_order.nil?
      orders = SuggestOptions.pairs("column_date_orders", column_date_orders)
      orders.each { |_ordinal, order| HyperCast::DATE_ORDERS.fetch(order) }
      super(sample_rows: [sample_rows, 1].max, format: format, date_order: date_order, column_date_orders: orders,
            column_formats: SuggestOptions.pairs("column_formats", column_formats))
    end

    # +entries+ as a frozen Array of [ordinal, value] pairs.
    def self.pairs(field, entries)
      entries.map do |entry|
        raise ArgumentError, "#{field} must hold [ordinal, value] pairs; got #{entry.inspect}" unless
          entry.is_a?(Array) && entry.size == 2 && entry[0].is_a?(Integer) && entry[0] >= 0

        entry.dup.freeze
      end.freeze
    end

    # The format declared for source column +ordinal+, or nil.
    def declared_format(ordinal)
      column_formats.reverse_each { |at, declared| return declared if at == ordinal }
      format
    end

    # The date order declared for source column +ordinal+, or nil.
    def declared_order(ordinal)
      column_date_orders.reverse_each { |at, order| return order if at == ordinal }
      date_order
    end

    # ArgumentError for something that isn't SuggestOptions, or for the first format given that the
    # core would refuse, naming its ordinal: the first thing a suggest does, before any source is
    # opened.
    def self.judged(options)
      raise ArgumentError, "options must be a HyperTabular::SuggestOptions; got #{options.inspect}" unless
        options.is_a?(SuggestOptions)

      options.check_formats
    end

    # ArgumentError for the first format given that the core would refuse, naming its ordinal.
    def check_formats
      declared = format.nil? ? column_formats : [[0, format], *column_formats]
      declared.each do |ordinal, candidate|
        Column.specs([Column.i32(0, candidate)])
      rescue ArgumentError => e
        raise ArgumentError, e.message.sub("plan column 0", "plan column #{ordinal}")
      end
      nil
    end
  end

  # Reads one thousand rows, declaring nothing.
  SuggestOptions::DEFAULT = SuggestOptions.new

  # What a sample says of one source column.
  #
  # +ordinal+ is the zero-based source column, as a Column names it, and +name+ the header's name
  # for it, or nil. +door+ is the door found (a key of DOORS), text when unresolved and under the
  # first candidate when ambiguous, and +declared+ what that door declares. +format+ is the notation
  # the plan reads a resolved numeric column in: the one declared for it, or else the one candidate
  # the sample left, or otherwise HyperCast::NumFormat::INVARIANT; both of those are Western, so a
  # later 1,23,456 is refused. nil for any other column. +candidates+ is what to choose between,
  # empty unless ambiguous: date orders as HyperCast names them, or HyperCast::NumFormats. Declare
  # one in SuggestOptions and ask again, since the door can differ under each. +status+ is how far
  # the sample settles it: :resolved (one door and one declaration take every cell, and the plan
  # reads the column through them), :ambiguous (every cell is read, but the sample can't say which
  # declaration it's written in, as 1,234 can't) or :unresolved (nothing to go on, or a cell no door
  # took beside cells one did). +matched+ is the cells +door+ took: resolved or ambiguous, all of
  # +sampled+; unresolved, the most any one door but the text door took. +sampled+ is the cells that
  # aren't blank.
  SuggestedColumn = Data.define(:ordinal, :name, :door, :declared, :format, :candidates, :status, :matched, :sampled)

  # What DelimitedReader.suggest, DelimitedReader.suggest_string and Sheet#suggest found.
  #
  # +plan+ is a frozen Array with a Column for each source column, in order, to change or to hand to
  # a read: only a resolved column reads through its door, and the rest read as text, which refuses
  # nothing and loses nothing. +report+ is a SuggestedColumn for each, in the plan's order.
  # +truncated+ says whether the source has rows past the sample: a source with exactly the sample's
  # rows hasn't.
  Suggestion = Data.define(:plan, :report, :truncated)

  # The sample, in the shape the core takes it, and what is made of the core's answer. Rust's
  # suggest.rs is where this comes from.
  class Sample
    # The one notation a suggestion can name besides Western and what's declared.
    COMMA_DECIMAL = HyperCast::NumFormat.new(decimal_sep: ",", group_sep: ".", flags: HyperCast::ALL_STYLES)

    # What the core's status codes mean.
    STATUSES = { 1 => :resolved, 2 => :ambiguous, 3 => :unresolved }.freeze

    # The notation bits in an ambiguous number's param, and a resolved one's value.
    NOTATIONS = { 2 => HyperCast::NumFormat::INVARIANT, 4 => COMMA_DECIMAL }.freeze

    # A span holds a u32 offset. A sample that far along stops, and says it did.
    MAX_BYTES = 0xFFFF_FFFF

    class << self
      # A delimited sample: the block opens a reader of the given batch size on the source, and is
      # called twice for a source with no header, whose width is its first record's.
      def delimited(dialect, options)
        limit = [options.sample_rows, DelimitedReader::DEFAULT_BATCH_ROWS].min
        width = nil
        unless dialect.has_header
          first = yield 1
          begin
            width = first.send(:first_width)
          ensure
            first.close
          end
        end
        reader = yield 1
        begin
          width ||= reader.header.size
          check_width(width)
          sample = new(width, limit)
          unless width.zero?
            # One row more than the sample, so that a row past it costs no second batch, in as many
            # batches as the core says that takes.
            reader.send(:sample_in, Array.new(width) { |ordinal| Column.text(ordinal) }, limit + 1)
            sample.read_from(reader)
          end
          sample.judge(reader.header, options)
        ensure
          reader.close
        end
      end

      # The plan a sheet's sample is read through: a text column for each source column, and after
      # them a UUID column for each, which takes only a text cell, so a typed cell says so.
      def sheet_plan(width)
        Array.new(width) { |ordinal| Column.text(ordinal) } + Array.new(width) { |ordinal| Column.uuid(ordinal) }
      end

      # ArgumentError for a source wider than a plan can say.
      def check_width(width)
        return if width <= Column::MAX_ORDINAL + 1

        raise ArgumentError,
              "plan column #{Column::MAX_ORDINAL + 1} cannot be honoured: the source has #{width} columns"
      end
    end

    # A sample of +width+ columns and at most +limit+ rows, with +typed+ a sheet's marks.
    def initialize(width, limit, typed: false)
      @width = width
      @limit = limit
      @typed = typed ? "".b : nil
      @bytes = "".b
      @spans = Array.new(width) { [] }
      @marks = Array.new(width) { "".b }
      @rows = 0
      @truncated = false
    end

    # Reads until the sample is whole and a row past it, or the end, says whether it's the source's
    # last. A failure among the sampled rows is the caller's, and a failure just past them, of the
    # source or of its structure, only says there's more.
    def read_from(source)
      until @truncated
        begin
          batch = source.read
        rescue TabularError, IOError, SystemCallError
          raise unless @rows == @limit

          @truncated = true
          break
        end
        return if batch.nil?

        take(batch)
      end
    end

    # Asks the core, and builds the plan and the report from its answer.
    def judge(header, options)
      declared = Array.new(@width) { |ordinal| options.declared_format(ordinal) }
      formats = declared.map { |format| format.nil? ? Column::NO_FORMAT : format.packed }.join
      cells = @spans.flatten.pack("L<*")
      typed = @typed && @marks.join
      found = Runtime.suggest(@bytes, cells, typed, @rows, @width, formats)
      # Every format was checked and every span was made here.
      raise ArgumentError, "plan column 0 cannot be honoured: the core refuses the sample" if found.nil?

      columns = found.each_slice(5).with_index.map do |said, ordinal|
        column_of(ordinal, header&.at(ordinal), declared[ordinal], options.declared_order(ordinal), *said)
      end
      Suggestion.new(plan: columns.map(&:first).freeze, report: columns.map(&:last).freeze, truncated: @truncated)
    end

    private

    # Takes what the sample still wants of +batch+. A blank, a token and padding are all a blank
    # span. A refused cell keeps its bytes, for the core's own text door to refuse.
    def take(batch)
      texts = Array.new(@width) { |column| batch.values(column) }
      batch.rows.times do |row|
        if @rows == @limit
          @truncated = true
          return
        end
        cells = Array.new(@width) { |column| texts[column][row] || refused(batch, column, row) }
        if @bytes.bytesize + cells.sum(&:bytesize) > MAX_BYTES
          @truncated = true
          return
        end
        cells.each_with_index do |cell, column|
          @spans[column] << @bytes.bytesize << cell.bytesize
          @bytes << cell.b
        end
        mark(batch, row) if @typed
        @rows += 1
      end
    end

    # The bytes of a cell the text door refused: none for a blank.
    def refused(batch, column, row)
      batch.get(column, row).reason == :empty ? "" : batch.raw(column, row)
    end

    # A cell the UUID door refuses with no detail isn't text: it's typed.
    def mark(batch, row)
      @width.times do |column|
        verdict = batch.get(@width + column, row)
        typed = verdict.is_a?(HyperCast::Fault) && verdict.reason == :malformed && verdict.detail.nil?
        @marks[column] << (typed ? 1 : 0)
      end
    end

    # One column of the core's answer as a plan column and a report entry.
    def column_of(ordinal, name, format, order, door, param, status, matched, sampled)
      status = STATUSES.fetch(status, :unresolved)
      candidates = status == :ambiguous ? candidates_of(door, param) : []
      # An ambiguous date's param is a set: the door it names is the first of the set's orders.
      said = Column.of_code(ordinal, door, status == :ambiguous ? lowest_bit(param) : param) || Column.text(ordinal)
      # An order declared for a date the sample left open is the answer to its question.
      if order && candidates.include?(order)
        status = :resolved
        candidates = []
        said = Column.date_ordered(ordinal, order)
      end
      numeric = status == :resolved && Column::NUMERIC.include?(said.door)
      planned = numeric ? format || (param == 4 ? COMMA_DECIMAL : HyperCast::NumFormat::INVARIANT) : nil
      plan = status == :resolved ? said.with(format: planned) : Column.text(ordinal)
      report = SuggestedColumn.new(ordinal: ordinal, name: name, door: said.door, declared: said.declared,
                                   format: planned, candidates: candidates.freeze, status: status,
                                   matched: matched, sampled: sampled)
      [plan, report]
    end

    # What an ambiguous column's param offers: date orders for a date door, notations otherwise.
    def candidates_of(door, param)
      if [HyperTabular::DOORS.fetch(:date_ordered), HyperTabular::DOORS.fetch(:datetime)].include?(door)
        HyperCast::DATE_ORDERS.select { |_order, code| param.anybits?(1 << code) }.keys
      else
        NOTATIONS.select { |bit, _format| param.anybits?(bit) }.values
      end
    end

    # The lowest bit set in +param+, as its number.
    def lowest_bit(param)
      param.zero? ? 0 : (param & -param).bit_length - 1
    end
  end
end
