module HyperTabular
  # Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time
  # into typed columns, every cell a HyperCast verdict.
  #
  #   plan = [HyperTabular::Column.i32(0), HyperTabular::Column.text(1), HyperTabular::Column.f64(2)]
  #   HyperTabular::DelimitedReader.open("orders.csv", HyperTabular::Dialect::CSV, plan) do |reader|
  #     while (batch = reader.read)
  #       ids = batch.values(0)                        # a column at a time: [1, 2, nil, 4, ...]
  #       batch.rows.times do |row|
  #         case batch.get(2, row)                     # or a cell at a time, as HyperCast's union
  #         in HyperCast::Success(value:) then total += value
  #         in HyperCast::Fault(reason:) then warn "#{reason}: #{batch.raw(2, row).inspect}"
  #         end
  #       end
  #     end
  #   end
  #
  # The native core (libhypertabular) owns no memory and reads no files. It is handed a
  # chunk of input and the buffers to fill, casts each plan column in one native loop, and
  # says how many rows it wrote and how many bytes it is finished with. Everything else is
  # here: this class holds the input, has one value array and one verdict array per column
  # allocated once and reused for every batch, and puts what the core did not consume back
  # in front of it. The native boundary is crossed once per batch, not once per cell, and a
  # column comes out of its buffer in one String#unpack.
  #
  # A value that does not cast is that cell's verdict, and the read goes on. Input that is
  # not rows of cells at all — a record of the wrong width, a quote never closed — is a
  # TabularError, raised after every intact row before it has been delivered.
  #
  # The plan may also wait for the header. Opened without one, the reader reads the header
  # and stops: its names say where each column is, and #bind declares the plan built from
  # them, once, before the first read:
  #
  #   reader = HyperTabular::DelimitedReader.open("orders.csv", HyperTabular::Dialect::CSV)
  #   header = reader.header                           # => ["id", "name", "score"]
  #   reader.bind([HyperTabular::Column.i32(header.ordinal("id")),
  #                HyperTabular::Column.f64(header.ordinal("score"))])
  #   reader.each { |row| total += row.value(1) || 0 } # a Row at a time, batch after batch
  #
  # Each #read returns a Batch, which owns what it shows: it stays good after the next
  # #read. Not thread-safe.
  #
  # A reader is called into once at a time: a call back into it from the IO it reads or from a
  # plan's #each, while it is reading or binding, raises RuntimeError naming it busy, and the call
  # it came from goes on as it was. An exception that lands in a read (Timeout.timeout,
  # Thread#raise, Ctrl-C's Interrupt) waits until the core's step and the reader's own state agree,
  # and a batch it lands on is handed over by the next #read, numbered the same. Only an IO read
  # that blocks is cut part-way, and since Ruby may drop what that read had taken from the IO, every
  # later read then raises RuntimeError naming the exception. Once the IO has answered, the
  # exception waits until its answer is held.
  class DelimitedReader
    include Enumerable
    include Guarded

    # The row ceiling: a single record larger than this is a :row_too_long TabularError.
    MAX_ROW_BYTES = 1 << 30

    # How much is asked of an IO at a time, until a record does not fit.
    DEFAULT_BUFFER_BYTES = 256 * 1024

    # Rows per batch unless told otherwise.
    DEFAULT_BATCH_ROWS = 4096

    # Encodings whose bytes already are the UTF-8 (or byte-identical) form the core reads.
    BYTE_COMPATIBLE = HyperCast::Interop::BYTE_COMPATIBLE

    # Opens a file of delimited text, in the dialect's encoding, through +plan+ — or, with none,
    # header first, for #bind. With a block, yields the reader, closes it — and the file — when
    # the block ends, and returns the block's value; without one, returns the reader, whose
    # #close closes the file.
    def self.open(path, dialect, plan = nil, batch_rows: DEFAULT_BATCH_ROWS, buffer_bytes: DEFAULT_BUFFER_BYTES,
                  notes: true)
      judged(dialect, plan, batch_rows, buffer_bytes, notes)
      file = File.open(path, "rb")
      begin
        reader = new(file, dialect, plan, batch_rows: batch_rows, buffer_bytes: buffer_bytes, close_source: true,
                                          notes: notes)
      ensure
        # The file is ours to close when no reader came of it, whatever went wrong.
        file.close if reader.nil?
      end
      return reader unless block_given?

      begin
        yield reader
      ensure
        reader.close
      end
    end

    # The core's answer on these sizes, ahead of touching a file. A plan that can't be honoured says
    # so where it always has, so its size is judged without it.
    private_class_method def self.judged(dialect, plan, batch_rows, buffer_bytes, notes)
      raise ArgumentError, "dialect must be a HyperTabular::Dialect; got #{dialect.inspect}" unless
        dialect.is_a?(Dialect)

      specs = begin
        plan.nil? ? [] : Column.specs(Column.plan(plan))
      rescue ArgumentError
        []
      end
      Runtime.check_sizes(specs, batch_rows, buffer_bytes, Runtime.batch_flags(notes, dialect.long_rows == :report))
    end

    # Offers a plan for the delimited file at +path+ from a sample, and applies nothing: a
    # Suggestion to inspect, change and hand to a read. Declaring the locale (a format and a date
    # order in +options+, a SuggestOptions) settles what a sample can't: `1.234` alone can't say
    # which it is. A format the core would refuse is an ArgumentError before the file is opened,
    # and a structural failure among the sampled rows a TabularError.
    def self.suggest(path, dialect, options = SuggestOptions::DEFAULT)
      SuggestOptions.judged(options)
      Sample.delimited(dialect, options) { |rows| self.open(path, dialect, batch_rows: rows, notes: false) }
    end

    # DelimitedReader.suggest, over a String.
    def self.suggest_string(text, dialect, options = SuggestOptions::DEFAULT)
      SuggestOptions.judged(options)
      Sample.delimited(dialect, options) { |rows| new(text, dialect, batch_rows: rows, notes: false) }
    end

    # Reads delimited text from +source+: a String, read in place when it is UTF-8 — nothing
    # is copied — or an IO (anything with #readpartial or #read), read forward only through a
    # buffer of +buffer_bytes+ that grows when a record does not fit it, one read of the IO each
    # time the core wants more, so finished rows reach the caller while a stream goes quiet. An
    # IO is read as bytes; one opened in text mode on Windows should be in binmode first. A
    # Dialect that declares an encoding has its bytes decoded ahead of the read, which a String
    # is copied for. An exception the IO raises goes to the caller and is final: every later
    # #read raises RuntimeError naming it.
    #
    # +dialect+ is the declared Dialect and +plan+ the output Columns, in output order — or
    # nil, for a reader that reads its header first and is given its plan by #bind.
    # +batch_rows+ is the most rows one #read delivers. +close_source+ says whether #close
    # also closes the IO. +notes+, on by default, is whether each cell's detail and origin are
    # kept: off, a Fault has no detail, Batch#origin says :cell, and Batch#faults lists no uncached
    # formula, for a read a little faster on a file full of bad cells.
    #
    # A dialect the scanner cannot honour, or a plan that is not Columns, is an
    # ArgumentError. With a header declared the header record is read here, so a header
    # that is structurally broken is a TabularError here.
    def initialize(source, dialect, plan = nil, batch_rows: DEFAULT_BATCH_ROWS, buffer_bytes: DEFAULT_BUFFER_BYTES,
                   close_source: false, notes: true)
      raise ArgumentError, "dialect must be a HyperTabular::Dialect; got #{dialect.inspect}" unless
        dialect.is_a?(Dialect)

      flags = Runtime.batch_flags(notes, dialect.long_rows == :report)
      start = Runtime.check_sizes([], batch_rows, buffer_bytes, flags)
      planned = Column.plan(plan) unless plan.nil?
      @dialect = dialect
      @batch_rows = batch_rows
      @buffer_bytes = start
      @close_source = close_source
      @notes = notes ? true : false
      @kernel = Runtime::Delimited.start(dialect.packed_with, dialect.packed_tokens)
      raise ArgumentError, "separator #{dialect.separator.inspect} is not tab or printable ASCII other than '\"'" if
        @kernel.nil?

      @plan = UNBOUND
      # A plan handed over here is bound before the header is read, as #bind would bind it.
      bind_kernel(planned, Column.specs(planned)) if planned
      @start = 0
      @closed = false
      @failure = nil
      # What the IO answered and the decoder made of it, each held until the buffer it goes into is
      # whole; and the batch made, until it is handed over.
      @incoming = @undelivered = @handing = @in_flight = nil
      take(source)
      @header = dialect.has_header ? read_header : nil
      # A plan bound before the header was read is held to it as soon as it is.
      within_header(planned) if planned
    end

    # The header's names, when the dialect declares a header: a Header — the frozen Array of
    # frozen UTF-8 Strings, quotes resolved, that also says where a name is
    # (Header#ordinal). Empty for an input with no record at all; nil when the dialect
    # declares no header.
    attr_reader :header

    # The plan: the output Columns, in output order. Column +i+ of every batch is +plan[i]+.
    # Empty until one is bound.
    attr_reader :plan

    # Declares the plan a reader opened without one reads through — +plan+, the output
    # Columns, in output order, typically built from the #header's ordinals — once, before
    # the first read. Returns the reader.
    #
    # ArgumentError for a plan that is not Columns, or that reads a column at or past the
    # width the header gives every record, which leaves the reader unbound; RuntimeError if
    # the reader already has a plan; IOError on a closed reader.
    def bind(plan)
      exclusive do
        raise IOError, "closed reader" if @closed
        raise "the reader already has a plan: a plan is bound once" if bound?

        plan = Column.plan(plan)
        specs = Column.specs(plan)
        within_header(plan)
        atomically { bind_kernel(plan, specs) }
        self
      end
    end

    # Whether a plan has been bound: always, for a reader opened with one.
    def bound?
      !@plan.equal?(UNBOUND)
    end

    # How many cells a record has: the header's count once it has been read, otherwise the
    # first record's once it has been; nil before either.
    def column_count
      expected = @kernel.expected
      expected.zero? ? nil : expected
    end

    # The declared Dialect.
    attr_reader :dialect

    # The most rows one #read delivers.
    attr_reader :batch_rows

    # Whether each cell's detail and origin are kept.
    attr_reader :notes

    # Records finished so far — the header and skipped blank lines included.
    def records
      @kernel.records
    end

    # Reads the next batch: a Batch of up to #batch_rows rows, or nil once the input is
    # exhausted. A TabularError when the input is structurally broken — raised after every
    # intact row before the break has been delivered, and the same error again on every
    # later call. Whatever the IO raises, unchanged, and then RuntimeError naming it on every later
    # call. IOError on a closed reader; RuntimeError before a plan is bound, which #bind then
    # cures, and from inside one of this reader's own calls (busy).
    def read
      handing_out do
        raise IOError, "closed reader" if @closed
        next hand_over if @handing
        raise @failure if @failure
        raise UNBOUND_MESSAGE unless bound?

        advance(false) ? hand_over : nil
      end
    end

    # Every remaining row, batch after batch: yields one frozen Array per row, a verdict
    # per plan column. Without a block, an Enumerator. Raises what #read raises.
    def each_row
      return to_enum(:each_row) unless block_given?

      batches do |batch|
        columns = Array.new(@plan.size) { |column| batch.verdicts(column) }
        batch.rows.times { |row| yield columns.map { |column| column[row] }.freeze }
      end
    end

    # Every remaining row, batch after batch, as a Row — which, its batch being a copy,
    # stays good after the read has moved on. Returns the reader; without a block, an
    # Enumerator, and the reader is Enumerable through it. Raises what #read raises.
    def each(&block)
      return to_enum(:each) unless block

      batches { |batch| batch.each(&block) }
    end

    # Closes the IO if this reader was told to (+close_source+, or DelimitedReader.open).
    # Batches already read stay good. Safe to call twice.
    def close
      exclusive do
        next if @closed

        @closed = true
        @in_flight = nil
        @io.close if @close_source && @io
      end
      nil
    end

    # True once #close has been called.
    def closed?
      @closed
    end

    # The reader in a line — not its buffers.
    def inspect
      "#<#{self.class.name} columns=#{@plan.size} records=#{records}#{' unbound' unless bound?}" \
        "#{' closed' if @closed}>"
    end

    private

    # The plan of a reader that has none yet.
    UNBOUND = [].freeze
    # What a read before a plan is bound says.
    UNBOUND_MESSAGE = "the reader has no plan yet: bind one before reading".freeze
    private_constant :UNBOUND, :UNBOUND_MESSAGE

    # The step ahead of the fill, for a dialect that declares an encoding: the original bytes, and
    # what the core's transcoder has made of them. A failure it meets is held until the fill has
    # had all the text ahead of it.
    class Decoder
      # The original offset of a unit with no character, once one has been met.
      attr_reader :failed_at

      # Decodes +encoding+ (the core's code). A String is all of the +input+; otherwise #feed gives
      # it each read of the source.
      def initialize(encoding, input)
        @encoding = encoding
        @raw = input || "".b
        @at = 0
        @eof = input ? true : false
        @offset = 0
        @failed_at = nil
      end

      # At most +room+ more bytes of decoded text, which is empty when there is no more until the
      # failure is raised or the source is over; nil when it needs more of the source first (#feed).
      # +room+ is at least the 4 one character takes.
      def step(room)
        consumed, written, failed, failure_at, out =
          Runtime.transcode(@encoding, @raw, @at, @raw.bytesize - @at, @eof, room)
        @failed_at = @offset + failure_at unless failed.zero?
        @at += consumed
        @offset += consumed
        out if written.positive? || @failed_at || @eof
      end

      # Keeps what is left, less than a character, and +chunk+ behind it: a read of the source, empty
      # at its end.
      def feed(chunk)
        if chunk.empty?
          @eof = true
          return
        end

        @raw = @raw.byteslice(@at, @raw.bytesize - @at).b << chunk.b
        @at = 0
      end

      # Whether the whole source has been decoded.
      def finished?
        @eof && @at == @raw.bytesize
      end
    end
    private_constant :Decoder

    # ArgumentError for the first column of +plan+ at or past the width the header has given
    # every record: such a column could only ever be empty.
    def within_header(plan)
      width = @header.nil? ? nil : column_count
      return if width.nil?

      index = plan.index { |column| column.ordinal >= width }
      return if index.nil?

      raise ArgumentError, "plan column #{index} reads source column #{plan[index].ordinal}, " \
                           "past the #{width} columns the header gives every record"
    end

    # Sizes the kernel's arrays for +plan+ (already checked to be Columns) and takes it. Only a
    # dialect that reports long rows keeps each row's width.
    def bind_kernel(plan, specs)
      # Cell-table entries one row takes: the widest ordinal the plan reads, plus two.
      per_row = (plan.map(&:ordinal).max || -1) + 2
      widths = @dialect.long_rows == :report
      Runtime.check_sizes(specs, @batch_rows, @buffer_bytes, Runtime.batch_flags(@notes, widths))
      @kernel.bind(specs, plan.map(&:value_bytes), @batch_rows, per_row, @notes, widths)
      @plan = plan
    end

    # Takes the source: a String becomes the whole input, an IO the thing to refill from.
    def take(source)
      encoding = Dialect::ENCODINGS.fetch(@dialect.encoding)
      if source.is_a?(String)
        @buffer = encoding.zero? ? utf8(source) : String.new(encoding: Encoding::UTF_8)
        @decoder = Decoder.new(encoding, source.b.freeze) unless encoding.zero?
        @eof = encoding.zero?
      elsif defined?(::Pathname) && source.is_a?(::Pathname)
        # It has a #read, and one that starts the file over on every call.
        raise ArgumentError, "source is a Pathname; DelimitedReader.open reads a path"
      elsif source.respond_to?(:readpartial) || source.respond_to?(:read)
        @io = source
        @partial = source.respond_to?(:readpartial)
        @buffer = String.new(encoding: Encoding::UTF_8)
        @eof = false
        @decoder = Decoder.new(encoding, nil) unless encoding.zero?
      else
        raise ArgumentError, "source must be a String or an IO; got #{source.class}"
      end
      @kernel.attach(@buffer)
    end

    # The text as a frozen String of UTF-8 bytes that is this reader's alone: the caller's
    # own when it is already that; otherwise a copy-on-write duplicate, which shares the
    # bytes and is immune to what the caller does to theirs next. Only a foreign encoding
    # pays a transcode.
    def utf8(text)
      return text if text.frozen? && text.encoding == Encoding::UTF_8
      return text.dup.force_encoding(Encoding::UTF_8).freeze if BYTE_COMPATIBLE.include?(text.encoding)

      text.encode(Encoding::UTF_8).freeze
    end

    # What the core is to read next — how many bytes from @start — and whether nothing
    # follows them.
    def window
      length = @buffer.bytesize - @start
      length > MAX_ROW_BYTES ? [MAX_ROW_BYTES, false] : [length, @eof]
    end

    # Runs the core over the buffer until it has a batch (held in @handing, for #hand_over) or, for
    # the +header+, the names: answers it, or false once the input is over. Only the IO's own read is
    # outside the wait for asynchronous exceptions, and only while it blocks (#fetch).
    def advance(header)
      loop do
        found = atomically { step(header) }
        return found unless found.is_a?(Integer)

        fetch(found)
      end
    end

    # One step of #advance, inside the wait: what it answers, or how many bytes to ask the IO for
    # before the next.
    def step(header)
      loop do
        length, last = window
        offset = @kernel.offset
        code = moving(offset) { header ? @kernel.header(@start, length, last) : @kernel.fill(@start, length, last) }
        raise structural unless code == Runtime::Delimited::OK

        found = moving(offset) do
          consumed = @kernel.consumed
          held = header ? header_names : made(@kernel.rows) if @kernel.rows.positive?
          @handing = held unless header || held.nil?
          @start += consumed
          next held unless held.nil?
          next false if last && (consumed == length || consumed.zero?)

          refill if consumed.zero?
        end
        return found unless found.nil?
      end
    end

    # Runs the block, which follows the core's call made at +offset+: an exception it raises once the
    # core has read on from there is kept as the reader's failure, since what the core read is lost.
    def moving(offset)
      yield
    rescue Exception => e
      @failure ||= spoiled(e) unless e.equal?(@failure) || @kernel.offset == offset
      raise
    end

    # The noun the busy and spoiled messages name.
    def noun = "reader"

    # Puts the unfinished record at the front of a new buffer, and what the IO answered behind it; or,
    # with nothing answered yet, how much to ask it for — at least as much again as is pending, so a
    # record that does not fit doubles the buffer rather than creeping up on it.
    def refill
      pending = @buffer.bytesize - @start
      return refill_decoded(pending) if @decoder

      raise too_long if @io.nil? || pending >= MAX_ROW_BYTES
      return [@buffer_bytes, pending].max if @incoming.nil?

      if @incoming.empty?
        @incoming = nil
        @eof = true
        return
      end
      rebuild(@buffer.byteslice(@start, pending) << @incoming.dup.force_encoding(Encoding::UTF_8))
      @incoming = nil
    end

    # As #refill, through the decoder. A failure it holds is raised once the fill has had all the
    # text ahead of it, and wants more: only a `\r` the fill is holding can still be finished, and
    # nothing is coming to say whether a `\n` is part of it, so this says so instead.
    def refill_decoded(pending)
      if @undelivered.nil? && (byte = @decoder.failed_at)
        unfinished = @buffer.byteslice(@start, pending)
        unless unfinished.end_with?("\r")
          raise @failure = TabularError.new(:encoding, @kernel.records, @kernel.line, byte)
        end

        return rebuild(unfinished << "\n")
      end
      raise too_long if pending >= MAX_ROW_BYTES

      # Decoded bytes are gone from the decoder, so they are held until the buffer they go into is
      # whole: an allocation refused along the way leaves a read that can be asked again.
      @undelivered ||= decoded([@buffer_bytes, pending, 4].max)
      return @buffer_bytes if @undelivered.nil?

      rebuild(@buffer.byteslice(@start, pending) << @undelivered.force_encoding(Encoding::UTF_8))
      @undelivered = nil
      @eof = @decoder.finished? && @decoder.failed_at.nil?
      nil
    end

    # The decoder's next text, given what the IO answered when it needs it; nil when it needs more.
    def decoded(room)
      loop do
        text = @decoder.step(room)
        return text unless text.nil? && @incoming

        @decoder.feed(@incoming)
        @incoming = nil
      end
    end

    # Attaches +grown+, a String no batch and no core call has seen, and makes it the buffer: the core
    # is never handed an address that a String it was reading no longer owns. Answers nil.
    def rebuild(grown)
      @kernel.attach(grown)
      @buffer = grown
      @start = 0
      nil
    end

    # Up to +bytes+ more of the IO, held in @incoming, empty at its end: #readpartial where there is
    # one, so a pipe or a socket hands over what it has rather than waiting to fill the request.
    #
    # An asynchronous exception lands only while the IO's read blocks, so a Timeout still breaks a
    # read waiting on a pipe; once the IO has answered it waits until the answer is held. One that
    # lands in the blocked read may take with it bytes Ruby had already read from the IO, so it
    # leaves the reader refusing every later read. So does anything the IO's read raises itself,
    # an IOError or a SystemCallError included: Ruby can't tell one from an exception that landed
    # there (Timeout.timeout's own class can be Errno::ETIMEDOUT), so none is taken as resumable.
    def fetch(bytes)
      answered = false
      while_blocked do
        @incoming = (@partial ? @io.readpartial(bytes) : @io.read(bytes)) || EMPTY
        answered = true
      rescue EOFError
        @incoming = EMPTY
        answered = true
      end
    rescue Exception => e
      Thread.handle_interrupt(Object => :never) { @failure ||= cut_short(e) } unless answered
      raise
    end

    # The failure that stands for +error+, raised while the IO was being read.
    def cut_short(error)
      RuntimeError.new("the reader can't read on: #{error.class} (#{error.message}) was raised while it was " \
                       "reading its IO, and what that read had taken from the IO may be lost")
    end

    # The end of the IO, held.
    EMPTY = "".b.freeze
    private_constant :EMPTY

    # The batch the core just wrote, copied out of the kernel's arrays: its spans index the
    # input from @start, where the core was handed it.
    def made(rows)
      columns = @kernel.columns
      reported = @dialect.long_rows == :report
      notes = @notes ? Array.new(@plan.size) { |column| columns.notes(column, rows) } : nil
      Batch.new(@plan, rows, Array.new(@plan.size) { |column| columns.values(column, rows) },
                Array.new(@plan.size) { |column| columns.verdicts(column, rows) },
                @kernel.cells, @kernel.per_row, @buffer, @start, @kernel.arena,
                workbook: false, notes: notes, widths: reported ? columns.widths(rows) : nil,
                expected: @kernel.expected, long_rows: Dialect::LONG_ROWS.fetch(@dialect.long_rows))
    end

    # Reads the header record: its names, or none for an input with no record at all, whose width
    # is unknown.
    def read_header
      advance(true) || Header.new.freeze
    end

    # The names the core just located: in the input as written, or — flagged — unescaped
    # in the arena.
    def header_names
      spans = @kernel.names
      arena = nil
      names = Header.new(@kernel.rows) do |index|
        offset = spans[index * 2]
        length = spans[index * 2 + 1]
        name =
          if length < Runtime::Delimited::SPAN_FLAG
            @buffer.byteslice(@start + offset, length)
          else
            arena ||= @kernel.arena.force_encoding(Encoding::UTF_8)
            arena.byteslice(offset, length & Runtime::Delimited::SPAN_LENGTH)
          end
        name.freeze
      end
      names.freeze
    end

    # The structural failure the core just reported, kept: it is final.
    def structural
      @failure = TabularError.from(@kernel.failure)
    end

    # What a sampler reads this reader in: batches of as many rows as the core says fit +wanted+.
    protected def sample_in(plan, wanted)
      @batch_rows = Runtime.sample_rows(Column.specs(plan), wanted,
                                        Runtime.batch_flags(@notes, @dialect.long_rows == :report))
      bind(plan)
    end

    # Binds a first read's one text column, one row a batch, and answers how many cells a record
    # has: how a suggestion learns a headerless source's width.
    protected def first_width
      bind([Column.text(0)])
      read
      column_count || 0
    end

    # The failure of a record that outgrew the ceiling, kept: it is final too.
    def too_long
      @failure = TabularError.new(:row_too_long, @kernel.records, @kernel.line, @kernel.offset)
    end
  end
end
