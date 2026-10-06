module HyperTabular
  # One sheet of a workbook, as Workbook#sheets lists it: its name, and whether the workbook
  # hides it. A hidden sheet reads like any other.
  SheetInfo = Data.define(:name, :hidden)

  # How a sheet is read: whether its first row is a header (exposed through Sheet#header
  # and never delivered as a row), whether a row with no cells is skipped, and the most rows
  # a batch holds.
  SheetOptions = Data.define(:has_header, :skip_empty_rows, :batch_rows) do
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
  #   sheet = book.sheet("Orders", HyperTabular::SheetOptions::DEFAULT, plan)
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

    # Opens the workbook in +bytes+, a String, which is read in place: a frozen String is
    # the workbook's as it is, anything else is copied once.
    def initialize(bytes)
      raise ArgumentError, "bytes must be a String; got #{bytes.class}" unless bytes.is_a?(String)

      bytes = bytes.b.freeze unless bytes.frozen?
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
    # +plan+, as +options+ (SheetOptions) says. With a header declared the header row is
    # read here. IndexError for an index the workbook has no sheet at, KeyError for a name
    # it has no sheet by.
    def sheet(which, options, plan)
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
  end

  # A forward-only read of one sheet of a Workbook, a batch at a time, through a plan — into
  # the same Batch delimited text is read into.
  #
  # A typed cell is converted directly by its door — a stored 42.0 never passes through text
  # to become an Integer — and a text cell goes through the door as delimited text would. A
  # sheet that is structurally broken raises a TabularError after every intact row before
  # the break has been delivered, and the same error again on every later read.
  class Sheet
    # The SheetOptions the sheet is read with.
    attr_reader :options

    # The plan: the output Columns, in output order.
    attr_reader :plan

    # The header row's names — a typed cell said the way the text door says it — as frozen
    # UTF-8 Strings, or nil when the options declare no header.
    attr_reader :header

    # Made by Workbook#sheet.
    def initialize(book, sheet, options, plan)
      raise ArgumentError, "options must be a HyperTabular::SheetOptions; got #{options.inspect}" unless
        options.is_a?(SheetOptions)

      @plan = Array(plan).dup.freeze
      @plan.each_with_index do |column, index|
        raise ArgumentError, "plan column #{index} must be a HyperTabular::Column; got #{column.inspect}" unless
          column.is_a?(Column)
      end
      @book = book
      @options = options
      width = (@plan.map(&:ordinal).max || -1) + 1
      @reading = Runtime::Book::Reading.new(book, sheet, options.has_header, options.skip_empty_rows,
                                            @plan.map(&:packed), @plan.map(&:value_bytes), options.batch_rows, width)
      @header = @reading.header
      @pending = nil
      @failure = nil
    rescue Runtime::Book::StructureError => e
      raise TabularError.from(e.failure)
    end

    # Reads the next batch: a Batch of up to the options' batch_rows rows, or nil once the
    # sheet has no more.
    def read
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

    # The sheet in a line.
    def inspect
      "#<#{self.class.name} columns=#{@plan.size}>"
    end
  end
end
