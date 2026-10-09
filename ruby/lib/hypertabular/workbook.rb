module HyperTabular
  # One sheet of a workbook, as Workbook#sheets lists it: its name, and whether the workbook
  # hides it. A hidden sheet reads like any other.
  SheetInfo = Data.define(:name, :hidden)

  # How a sheet is read: whether its first row is a header (exposed through Sheet#header
  # and never delivered as a row), whether a row with no cells is skipped, and the most rows
  # a batch holds.
  SheetOptions = Data.define(:has_header, :skip_empty_rows, :batch_rows) do
    # Every option has a default; ArgumentError unless +batch_rows+ is a positive Integer.
    def initialize(has_header: true, skip_empty_rows: true, batch_rows: DelimitedReader::DEFAULT_BATCH_ROWS)
      raise ArgumentError, "batch_rows must be a positive Integer; got #{batch_rows.inspect}" unless
        batch_rows.is_a?(Integer) && batch_rows.positive?

      super
    end
  end

  # A header, empty rows skipped, DelimitedReader::DEFAULT_BATCH_ROWS rows a batch.
  SheetOptions::DEFAULT = SheetOptions.new

  # An XLSX or ODS workbook held in memory, its sheets listed and its shared strings and
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

    # Which kind of workbook this is: :xlsx or :ods.
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
  class Sheet
    include Enumerable

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

      planned = Column.plan(plan) unless plan.nil?
      @book = book
      @options = options
      @plan = UNBOUND
      @reading = Runtime::Book::Reading.new(book, sheet, options.has_header, options.skip_empty_rows,
                                            options.batch_rows)
      # A plan handed over here is bound before the header is read, as #bind would bind it;
      # without one the header row is read into slots enough for every cell it has, and
      # binding later keeps them (a row the sheet repeats is delivered from them again).
      bind_reading(planned) if planned
      @header = options.has_header ? Header.new(@reading.read_header).freeze : nil
      @pending = nil
      @failure = nil
    rescue Runtime::Book::StructureError => e
      raise TabularError.from(e.failure)
    end

    # Declares the plan a sheet opened without one reads through — +plan+, the output
    # Columns, in output order, typically built from the #header's ordinals — once, before
    # the first read. Returns the sheet.
    #
    # ArgumentError for a plan that is not Columns, which leaves the sheet unbound;
    # RuntimeError if the sheet already has a plan.
    def bind(plan)
      raise "the sheet already has a plan: a plan is bound once" if bound?

      bind_reading(Column.plan(plan))
      self
    end

    # Whether a plan has been bound: always, for a sheet opened with one.
    def bound?
      !@plan.equal?(UNBOUND)
    end

    # Reads the next batch: a Batch of up to the options' batch_rows rows, or nil once the
    # sheet has no more. RuntimeError before a plan is bound, which #bind then cures.
    def read
      raise "the sheet has no plan yet: bind one before reading" unless bound?

      if @pending
        @failure = @pending
        @pending = nil
      end
      raise @failure if @failure

      rows, cells, arena, failure = @reading.fill
      if failure
        broken = TabularError.from(failure)
        raise @failure = broken if rows.zero?

        @pending = broken
      end
      return nil if rows.zero?

      columns = @reading.columns
      values = Array.new(@plan.size) { |column| columns.values(column, rows) }
      Batch.new(@plan, rows, values, Array.new(@plan.size) { |column| columns.verdicts(column, rows) },
                cells, @reading.per_row, @book.strings, 0, arena, workbook: true)
    end

    # Every remaining batch. Without a block, an Enumerator.
    def each_batch
      return to_enum(:each_batch) unless block_given?

      while (batch = read)
        yield batch
      end
      self
    end

    # Every remaining row, batch after batch, as a Row — which, its batch being a copy,
    # stays good after the read has moved on. Returns the sheet; without a block, an
    # Enumerator, and the sheet is Enumerable through it. Raises what #read raises.
    def each(&block)
      return to_enum(:each) unless block

      while (batch = read)
        batch.each(&block)
      end
      self
    end

    # The sheet in a line.
    def inspect
      "#<#{self.class.name} columns=#{@plan.size}#{' unbound' unless bound?}>"
    end

    private

    # The plan of a sheet that has none yet.
    UNBOUND = [].freeze
    private_constant :UNBOUND

    # Sizes the reading's arrays for +plan+ (already checked to be Columns) and takes it.
    def bind_reading(plan)
      # Slots the core assembles a row in: one for every source column the plan reaches.
      width = (plan.map(&:ordinal).max || -1) + 1
      @reading.bind(plan.map(&:packed), plan.map(&:value_bytes), width)
      @plan = plan
    end
  end
end
