module HyperTabular
  # One sheet of a workbook, as Workbook#sheets lists it: its name, and whether the workbook
  # hides it. A hidden sheet reads like any other.
  SheetInfo = Data.define(:name, :hidden)

  # A rectangle of a sheet to read, as a 1-based row and column on each side, 0 being no limit on
  # that side. A plan's ordinal 0 is its first column; Batch#line and Problem#source_column stay
  # the sheet's own. A range past the sheet's end invents no rows.
  #
  #   HyperTabular::Range.parse("A5:F200")
  #   HyperTabular::Range.new(first_row: 5, first_column: 1, last_row: 0, last_column: 6)
  Range = Data.define(:first_row, :first_column, :last_row, :last_column) do
    # ArgumentError for a side that isn't a count, or one whose first is past its last.
    def initialize(first_row: 0, first_column: 0, last_row: 0, last_column: 0)
      [[first_row, last_row, "row"], [first_column, last_column, "column"]].each do |first, last, side|
        Declared.count("first #{side}", first)
        Declared.count("last #{side}", last)
        raise ArgumentError, "the range's first #{side} #{first} is past its last, #{last}" if
          first.positive? && last.positive? && first > last
      end
      super
    end

    # The range +text+ says, in the spreadsheet's own spelling: capitals, both corners, the first
    # not past the last, as "A5:F200". ArgumentError for anything else.
    def self.parse(text)
      raise ArgumentError, "a range is a String; got #{text.inspect}" unless text.is_a?(String)

      corners = Runtime.range_parse(text)
      raise ArgumentError, "#{text.inspect} is not a range such as A5:F200" if corners.nil?

      new(first_row: corners[0], first_column: corners[1], last_row: corners[2], last_column: corners[3])
    end
  end

  # How a sheet is read: whether its first row is a header (exposed through Sheet#header
  # and never delivered as a row), whether an empty row is skipped, and the most rows a batch
  # holds.
  #
  # A row of only formulas with nothing cached counts as empty, so +skip_empty_rows+ filters it
  # by default: `skip_empty_rows: false` keeps every row. +skip+ is how many rows to pass over at
  # the range's start, empty ones included, their cells not judged. +header_row+ is how many rows
  # to read and discard after those, counted after the empty-row filter. +range+ is the Range to
  # read, or nil for the whole sheet. +tokens+ is what stands for no value in a text cell of every
  # column without tokens of its own. +notes+, on by default, is whether each cell's detail and
  # origin are kept: off, a Fault has no detail, Batch#origin says :cell, and Batch#faults lists
  # no uncached formula.
  SheetOptions = Data.define(:has_header, :skip_empty_rows, :batch_rows, :skip, :header_row, :range, :tokens,
                             :notes) do
    # Every option has a default; ArgumentError unless +skip+ and +header_row+ are counts, +range+
    # a Range or nil and +tokens+ an Array of Strings. The core judges +batch_rows+ when the sheet
    # is opened.
    def initialize(has_header: true, skip_empty_rows: true, batch_rows: DelimitedReader::DEFAULT_BATCH_ROWS,
                   skip: 0, header_row: 0, range: nil, tokens: [], notes: true)
      raise ArgumentError, "range must be a HyperTabular::Range; got #{range.inspect}" unless
        range.nil? || range.is_a?(Range)

      super(has_header: has_header, skip_empty_rows: skip_empty_rows, batch_rows: batch_rows,
            skip: Declared.count("skip", skip), header_row: Declared.count("header_row", header_row), range: range,
            tokens: Declared.tokens(tokens), notes: notes ? true : false)
    end

    # The 36 bytes of the core's RawSheetOptions.
    def packed_with
      [SheetOptions::OPTIONS_BYTES, has_header ? 1 : 0, skip_empty_rows ? 1 : 0, skip, header_row,
       range&.first_row.to_i, range&.first_column.to_i, range&.last_row.to_i, range&.last_column.to_i].pack("L<9")
    end

    # The tokens as the core takes them, or nil for none.
    def packed_tokens
      tokens.empty? ? nil : Declared.packed_tokens(tokens)
    end

    # The source column a plan's ordinal 0 is: the range's first, A being 0.
    def first_column
      [range&.first_column.to_i - 1, 0].max
    end
  end

  # The size of SheetOptions#packed_with: what RawSheetOptions' first word carries.
  SheetOptions::OPTIONS_BYTES = 36

  # A header, empty rows skipped, DelimitedReader::DEFAULT_BATCH_ROWS rows a batch.
  SheetOptions::DEFAULT = SheetOptions.new

  # An XLSX, ODS or Numbers workbook held in memory, its sheets listed and its shared strings and
  # styles loaded: what a Sheet reads from.
  #
  #   book = HyperTabular::Workbook.open("orders.xlsx")
  #   book.sheets                  # => [#<data HyperTabular::SheetInfo name="Orders", hidden=false>]
  #   sheet = book.sheet("Orders", HyperTabular::SheetOptions::DEFAULT)
  #   sheet.bind([HyperTabular::Column.i32(sheet.header.ordinal("id"))])
  #   while (batch = sheet.read) ... end
  #
  # A workbook that cannot be read — not a zip, encrypted, a part missing or broken — is a
  # TabularError from Workbook.new. Its sheets may be read at once, each with buffers of its
  # own. Not thread-safe.
  class Workbook
    # Reads the file at +path+ into memory and opens it.
    def self.open(path)
      new(File.binread(path))
    end

    # Opens the workbook in +source+: a String, which is read in place — a frozen String is
    # the workbook's as it is, anything else is copied once — or an IO (anything with
    # #read), read to its end into a String the workbook owns, as soon as it is opened; the
    # IO is closed once it has been read if +close_source+ says so. Which kind of workbook
    # it is is read from its bytes, never from a name.
    def initialize(source, close_source: false)
      bytes =
        if source.is_a?(String)
          source.frozen? ? source : source.b.freeze
        elsif defined?(::Pathname) && source.is_a?(::Pathname)
          # It has a #read, but it is a path, and Workbook.open is what reads one.
          raise ArgumentError, "source is a Pathname; Workbook.open reads a path"
        elsif source.respond_to?(:read)
          drain(source, close_source)
        else
          raise ArgumentError, "source must be a String or an IO; got #{source.class}"
        end
      @book = Runtime::Book::Opened.new(bytes)
      @sheets = @book.sheets.map { |name, hidden, _part, _index| SheetInfo.new(name: name, hidden: hidden) }.freeze
    rescue Runtime::Book::StructureError => e
      raise TabularError.from(e.failure)
    end

    # The workbook's sheets, in its own order, as SheetInfo. Sheets that hold no cells
    # (chart sheets, macro sheets) are not among them.
    attr_reader :sheets

    # Which kind of workbook this is: :xlsx, :ods or :numbers.
    def format
      @book.format
    end

    # The date system the workbook's serials count in — what a date-formatted number is
    # read by: :y1900 or :y1904, as HyperCast names them.
    def date_system
      HyperCast::EXCEL_EPOCHS.key(@book.epoch)
    end

    # Starts a read of one sheet — +which+ is its index in #sheets or its name — through
    # +plan+, as +options+ (SheetOptions) says; or, with no plan, header first, the plan to
    # be given by Sheet#bind once the header has said where each column is. With a header
    # declared the header row is read here. IndexError for an index the workbook has no
    # sheet at, KeyError for a name it has no sheet by.
    def sheet(which, options, plan = nil)
      index =
        case which
        when Integer
          raise IndexError, "the workbook has #{@sheets.size} sheets, and no sheet #{which}" unless
            which >= 0 && which < @sheets.size

          which
        when String
          found = @sheets.index { |sheet| sheet.name == which }
          raise KeyError, "the workbook has no sheet named #{which.inspect}" if found.nil?

          found
        else raise ArgumentError, "which must be an Integer or a String; got #{which.inspect}"
        end
      Sheet.new(@book, @book.sheets[index], options, plan)
    end

    # The workbook in a line.
    def inspect
      "#<#{self.class.name} #{format} sheets=#{@sheets.size}>"
    end

    private

    # Everything +io+ has left, as a frozen binary String of the workbook's own; the IO
    # closed after, if +close+ says so, whether or not the read went well.
    def drain(io, close)
      bytes = io.read
      bytes = bytes.nil? ? String.new : bytes.dup
      bytes.force_encoding(Encoding::BINARY).freeze
    ensure
      io.close if close
    end
  end

  # A forward-only read of one sheet of a Workbook, a batch at a time, through a plan — into
  # the same Batch delimited text is read into.
  #
  # A typed cell is converted directly by its door — a stored 42.0 never passes through text
  # to become an Integer — and a text cell goes through the door as delimited text would. A
  # sheet that is structurally broken raises a TabularError after every intact row before
  # the break has been delivered, and the same error again on every later read.
  #
  # A sheet is called into once at a time, as a DelimitedReader is: a call back into it from a
  # plan's #each while it binds raises RuntimeError naming it busy. An exception that lands in a
  # read (Timeout.timeout, Thread#raise, Ctrl-C's Interrupt) waits until the core's step and the
  # sheet's own state agree, and a batch it lands on is handed over by the next #read.
  class Sheet
    include Enumerable
    include Guarded

    # The SheetOptions the sheet is read with.
    attr_reader :options

    # The plan: the output Columns, in output order. Empty until one is bound.
    attr_reader :plan

    # The header row's names — a typed cell said the way the text door says it — as a
    # Header (the frozen Array of frozen UTF-8 Strings, which also says where a name is), or
    # nil when the options declare no header.
    attr_reader :header

    # Made by Workbook#sheet.
    def initialize(book, sheet, options, plan)
      raise ArgumentError, "options must be a HyperTabular::SheetOptions; got #{options.inspect}" unless
        options.is_a?(SheetOptions)

      Runtime.check_sizes([], options.batch_rows, 1, Runtime.batch_flags(options.notes, false, sheet: true))
      planned = Column.plan(plan) unless plan.nil?
      specs = Column.specs(planned) if planned
      @book = book
      @entry = sheet
      @options = options
      @plan = UNBOUND
      @reading = Runtime::Book::Reading.new(book, sheet, options.packed_with, options.batch_rows, options.packed_tokens)
      # A plan handed over here is bound before the header is read, as #bind would bind it;
      # without one the header row is read into slots enough for every cell it has, and
      # binding later keeps them (a row the sheet repeats is delivered from them again).
      bind_reading(planned, specs) if planned
      @header = options.has_header ? Header.new(@reading.read_header).freeze : nil
      @pending = nil
      @failure = nil
      @handing = @in_flight = nil
    rescue Runtime::Book::StructureError => e
      raise TabularError.from(e.failure)
    end

    # Declares the plan a sheet opened without one reads through — +plan+, the output
    # Columns, in output order, typically built from the #header's ordinals — once, before
    # the first read. Returns the sheet.
    #
    # ArgumentError for a plan that is not Columns or that the core refuses, which leaves the
    # sheet unbound; RuntimeError if the sheet already has a plan, and from inside one of this
    # sheet's own calls (busy).
    def bind(plan)
      exclusive do
        raise "the sheet already has a plan: a plan is bound once" if bound?

        plan = Column.plan(plan)
        specs = Column.specs(plan)
        atomically { bind_reading(plan, specs) }
        self
      end
    end

    # Whether a plan has been bound: always, for a sheet opened with one.
    def bound?
      !@plan.equal?(UNBOUND)
    end

    # Reads the next batch: a Batch of up to the options' batch_rows rows, or nil once the
    # sheet has no more. RuntimeError before a plan is bound, which #bind then cures, and from
    # inside one of this sheet's own calls (busy).
    def read
      handing_out do
        raise "the sheet has no plan yet: bind one before reading" unless bound?
        next hand_over if @handing

        if @pending
          @failure = @pending
          @pending = nil
        end
        raise @failure if @failure

        atomically { step } ? hand_over : nil
      end
    end

    # Offers a plan for this sheet from a sample, and applies nothing: a Suggestion to inspect,
    # change and hand to a read. It opens the sheet again, so what has been read of this one
    # doesn't matter. The options' batch_rows caps +options+' sample_rows, and the rows are read in
    # as many batches as the core says fit. A typed number has no notation to ask about, so it reads
    # by its value, whichever is declared; a sheet's date column suggests text. A format the core would refuse is an
    # ArgumentError before anything is read, and a structural failure among the sampled rows a
    # TabularError.
    def suggest(options = SuggestOptions::DEFAULT)
      exclusive do
        SuggestOptions.judged(options)
        limit = [options.sample_rows, @options.batch_rows].min
        # Notes on whatever this sheet says, for what the sample's second door says of a cell.
        read = @options.with(notes: true, batch_rows: 1)
        open = ->(opening) { Sheet.new(@book, @entry, opening, nil) }
        width = read.has_header ? nil : open.call(read).first_width
        sheet = open.call(read)
        width ||= sheet.header.size
        Sample.check_width(width)
        # One row more than the sample, so that a row past it costs no second batch, in as many batches
        # as the core says that takes.
        sheet.sample_in(Sample.sheet_plan(width), limit + 1) unless width.zero?
        sample = Sample.new(width, limit, typed: true)
        sample.read_from(sheet) unless width.zero?
        sample.judge(sheet.header, options)
      end
    end

    # Every remaining batch. Without a block, an Enumerator.
    def each_batch
      return to_enum(:each_batch) unless block_given?

      batches { |batch| yield batch }
    end

    # Every remaining row, batch after batch, as a Row — which, its batch being a copy,
    # stays good after the read has moved on. Returns the sheet; without a block, an
    # Enumerator, and the sheet is Enumerable through it. Raises what #read raises.
    def each(&block)
      return to_enum(:each) unless block

      batches { |batch| batch.each(&block) }
    end

    # The sheet in a line.
    def inspect
      "#<#{self.class.name} columns=#{@plan.size}#{' unbound' unless bound?}>"
    end

    # A first read's one text column, one row a batch, and the cells that row had: how a suggestion
    # learns a headerless sheet's width. 0 for a sheet with no row.
    protected def first_width
      plan = [Column.text(0)]
      bind_reading(plan, Column.specs(plan), widths: true)
      read.nil? ? 0 : @reading.columns.widths(1).unpack1("L<")
    end

    private

    # The plan of a sheet that has none yet.
    UNBOUND = [].freeze
    private_constant :UNBOUND

    # What a sampler reads this sheet in: batches of as many rows as the core says fit +wanted+.
    protected def sample_in(plan, wanted)
      flags = Runtime.batch_flags(@options.notes, false, sheet: true)
      @options = @options.with(batch_rows: Runtime.sample_rows(Column.specs(plan), wanted, flags))
      bind(plan)
    end

    # The noun the busy and spoiled messages name.
    def noun = "sheet"

    # One fill and the batch made of it, inside the wait for asynchronous exceptions: true with the
    # batch held in @handing, or false once the sheet has no more. The core can't be asked again for
    # rows it has gone past, so an exception raised along the way is kept as the sheet's failure.
    def step
      rows, cells, arena, failure = @reading.fill
      if failure
        broken = TabularError.from(failure)
        raise @failure = broken if rows.zero?

        @pending = broken
      end
      return false if rows.zero?

      columns = @reading.columns
      values = Array.new(@plan.size) { |column| columns.values(column, rows) }
      notes = @options.notes ? Array.new(@plan.size) { |column| columns.notes(column, rows) } : nil
      @handing = Batch.new(@plan, rows, values, Array.new(@plan.size) { |column| columns.verdicts(column, rows) },
                           cells, @reading.per_row, @book.strings, 0, arena,
                           workbook: true, notes: notes, first_column: @options.first_column)
      true
    rescue Exception => e
      unless e.equal?(@failure)
        @pending = nil
        @failure = spoiled(e)
      end
      raise
    end

    # Sizes the reading's arrays for +plan+ (already checked to be Columns) and takes it, keeping
    # each row's width if a caller here wants it.
    def bind_reading(plan, specs, widths: false)
      # Slots the core assembles a row in: one for every source column the plan reaches.
      width = (plan.map(&:ordinal).max || -1) + 1
      Runtime.check_sizes(specs, @options.batch_rows, 1, Runtime.batch_flags(@options.notes, widths, sheet: true))
      @reading.bind(specs, plan.map(&:value_bytes), width, @options.notes, widths)
      @plan = plan
    end
  end
end
