# A call back into a reader or a sheet from inside one of its own calls, and an exception that
# lands in the middle of one: refused as busy, or every row delivered once with its own line, or
# every later read refused saying why. Never a wrong row.

require "stringio"
require "timeout"
require "spec_helper"
require_relative "corpus_helpers"

RSpec.describe "a call back in, and an exception mid-call" do
  column = HyperTabular::Column
  reader = HyperTabular::DelimitedReader
  dialect_class = HyperTabular::Dialect
  workbooks = File.join(CorpusHelpers.directory, "workbook")
  count = 60
  text = "a,b\n#{(0...count).map { |row| "#{row},r#{row}\n" }.join}"
  plan = [column.i32(0), column.text(1)].freeze
  control = (0...count).map { |row| [row + 2, row.to_s, "r#{row}"] }
  long_text = "a,b\n#{(0...20).map { |row| "#{row},#{'w' * 40}#{row}\n" }.join}"
  long_control = (0...20).map { |row| [row + 2, row.to_s, "#{'w' * 40}#{row}"] }
  encodings = { utf8: "UTF-8", latin1: "ISO-8859-1", utf16le: "UTF-16LE" }
  busy = /\Athe (reader|sheet) is in the middle of a call; nothing that call runs can call back into it\z/

  # An IO handing out its bytes +size+ at a time, calling +hook+ with each read's index first.
  pieces = Class.new do
    def initialize(bytes, size, &hook)
      @bytes = bytes.b
      @size = size
      @hook = hook
      @at = 0
      @reads = 0
    end

    def readpartial(length)
      @reads += 1
      @hook&.call(@reads)
      raise EOFError if @at >= @bytes.bytesize

      part = @bytes.byteslice(@at, [length, @size].min)
      @at += part.bytesize
      part
    end
  end

  rows_of = ->(batch) { (0...batch.rows).map { |row| [batch.line(row), batch.raw(0, row), batch.raw(1, row)] } }

  # Every row a read hands over, reading on past each Interrupt, and how many it caught.
  drained = lambda do |held|
    out = []
    caught = 0
    loop do
      batch = begin
        held.read
      rescue Interrupt
        caught += 1
        next
      end
      break if batch.nil?

      out.concat(rows_of.call(batch))
    end
    [out, caught]
  end

  # Thread#raise from another thread, as Timeout.timeout and Ctrl-C deliver an exception: it lands
  # wherever this thread next lets it, which a wait (Thread.handle_interrupt) puts off.
  delivered = lambda do |error|
    main = Thread.current
    Thread.new { main.raise(error) }.join
  end

  dialect_of = ->(encoding) { dialect_class.new(separator: ",", encoding: encoding) }

  # An Interrupt delivered to this thread with nothing blocking along the way, so that it is pending
  # at the next point that checks: the return of the method that called this, unless held.
  delivered_unblocked = lambda do |error|
    main = Thread.current
    raiser = Thread.new { main.raise(error) }
    Thread.pass while raiser.alive?
  end

  describe "a call back in from the reader's own IO" do
    calls = {
      read: ->(held) { held.read },
      each: ->(held) { held.each { nil } },
      each_row: ->(held) { held.each_row { nil } },
      close: ->(held) { held.close },
      bind: ->(held) { held.bind([column.text(0)]) }
    }
    encodings.each do |encoding, codec|
      calls.each do |name, call|
        it "refuses #{name} as busy and reads on as it was, #{encoding}" do
          held = nil
          said = []
          io = pieces.new(text.encode(codec), 7) do |reads|
            next unless held && reads >= 4 && said.empty?

            call.call(held)
            said << :accepted
          rescue RuntimeError => e
            said << e.message
          end
          held = reader.new(io, dialect_of.call(encoding), plan, batch_rows: 3)
          out, = drained.call(held)
          expect(said.size).to eq(1)
          expect(said.first).to match(busy)
          expect(out).to eq(control)
          expect(held.closed?).to be(false)
        end
      end
    end

    it "takes a call from the caller's own block, between reads" do
      held = reader.new(text, dialect_class::CSV, plan, batch_rows: 3)
      seen = []
      held.each do |row|
        seen << row.value(0)
        next if seen.size < 3

        held.close
        break
      end
      expect(seen).to eq([0, 1, 2])
      expect(held.closed?).to be(true)
    end
  end

  # The plan an Enumerable gives, whose #each is the caller's code.
  planned = lambda do |calls, said, ordinal|
    Enumerator.new do |columns|
      calls.each do |call|
        call.call
        said << :accepted
      rescue RuntimeError => e
        said << e.message
      end
      columns << column.i32(ordinal)
    end
  end

  it "refuses a call back in from the plan a reader is binding" do
    held = reader.new("a,b\n1,x\n2,y\n", dialect_class::CSV)
    said = []
    held.bind(planned.call([-> { held.read }, -> { held.bind([column.text(1)]) }, -> { held.close }], said, 0))
    expect(said.size).to eq(3)
    expect(said).to all(match(busy))
    expect(held.plan.map(&:ordinal)).to eq([0])
    expect(held.read.values(0)).to eq([1, 2])
  end

  it "refuses a call back in from the plan a sheet is binding" do
    book = HyperTabular::Workbook.open(File.join(workbooks, "generated-a.xlsx"))
    sheet = book.sheet(0, HyperTabular::SheetOptions.new(has_header: false))
    said = []
    calls = [-> { sheet.read }, -> { sheet.bind([column.text(1)]) }, -> { sheet.suggest }]
    sheet.bind(planned.call(calls, said, 0))
    expect(said.size).to eq(3)
    expect(said).to all(match(/\Athe sheet is in the middle of a call; /))
    expect(sheet.plan.map(&:ordinal)).to eq([0])
    expect(sheet.read.rows).to be_positive
  end

  describe "an exception delivered as the IO answers" do
    # Pieces that deliver an Interrupt as read +armed+ answers, with nothing blocking on the way.
    answering = Class.new(pieces) do
      attr_accessor :armed
      attr_reader :reads

      define_method(:readpartial) do |length|
        super(length).tap { delivered_unblocked.call(Interrupt.new("as it answered")) if @reads == armed }
      end
    end

    encodings.each do |encoding, codec|
      it "keeps what the IO handed over, every read of it after the header, #{encoding}" do
        bytes = text.encode(codec)
        counted = answering.new(bytes, 13)
        opened = reader.new(counted, dialect_of.call(encoding), plan, batch_rows: 4)
        first = counted.reads
        drained.call(opened)
        expect(counted.reads - first).to be > 4
        # The last read is the IO's end, which raises EOFError rather than answering.
        ((first + 1)...counted.reads).each do |armed_at|
          io = answering.new(bytes, 13)
          held = reader.new(io, dialect_of.call(encoding), plan, batch_rows: 4)
          io.armed = armed_at
          out, caught = drained.call(held)
          expect([armed_at, out]).to eq([armed_at, control])
          expect(caught).to eq(1)
        end
      end
    end
  end

  describe "an exception delivered as a read hands its batch out" do
    # +held+, with an Interrupt delivered as the armed_at-th batch is let go of, with nothing blocking.
    arm_take = lambda do |held, armed_at|
      takes = 0
      held.singleton_class.prepend(Module.new do
        define_method(:let_go) do
          super().tap do
            takes += 1
            delivered_unblocked.call(Interrupt.new("as it was handed out")) if takes == armed_at
          end
        end
        private :let_go
      end)
      held
    end
    # +held+, with an Interrupt delivered as the read'th #read returns, past every frame of its own.
    arm_return = lambda do |held, armed_at|
      reads = 0
      held.singleton_class.prepend(Module.new do
        define_method(:read) do
          super().tap do
            reads += 1
            delivered_unblocked.call(Interrupt.new("as read returned")) if reads == armed_at
          end
        end
      end)
      held
    end
    sheet_of = lambda do
      book = HyperTabular::Workbook.open(File.join(workbooks, "generated-a.xlsx"))
      book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 3), [column.text(0), column.text(1)])
    end
    sources = {
      "reader over a String" => -> { reader.new(text, dialect_class::CSV, plan, batch_rows: 4) },
      "reader over an IO, latin1" => lambda {
        reader.new(pieces.new(text.encode("ISO-8859-1"), 13), dialect_of.call(:latin1), plan, batch_rows: 4)
      },
      "sheet" => sheet_of
    }
    rows_by_each = lambda do |held|
      out = []
      caught = 0
      begin
        held.each { |row| out << [row.line, row.raw(0), row.raw(1)] }
      rescue Interrupt
        caught += 1
        retry
      end
      [out, caught]
    end

    sources.each do |name, make|
      it "holds the batch again for the next read, #{name}" do
        expected, = drained.call(make.call)
        batches = expected.size.fdiv(name == "sheet" ? 3 : 4).ceil
        (1..batches).each do |armed_at|
          out, caught = drained.call(arm_take.call(make.call, armed_at))
          expect([armed_at, out]).to eq([armed_at, expected])
          expect(caught).to eq(1)
        end
      end

      it "holds the batch again when it lands between #each's read and its block, #{name}" do
        expected, = rows_by_each.call(make.call)
        batches = expected.size.fdiv(name == "sheet" ? 3 : 4).ceil
        (1..batches).each do |armed_at|
          out, caught = rows_by_each.call(arm_return.call(make.call, armed_at))
          expect([armed_at, out]).to eq([armed_at, expected])
          expect(caught).to eq(1)
        end
      end
    end

    it "gives nothing back for an exception from #each's own block, which had the batch" do
      held = reader.new(text, dialect_class::CSV, plan, batch_rows: 4)
      seen = []
      expect do
        held.each do |row|
          seen << [row.line, row.raw(0), row.raw(1)]
          raise Interrupt
        end
      end.to raise_error(Interrupt)
      out, = drained.call(held)
      lines = (seen + out).map(&:first)
      expect(lines).to eq(lines.uniq)
    end
  end

  # The Rust core hands out batches and keeps none of a batch's rows for the next read, and Python's
  # reader.rows() is the same: a walk left part-way through a batch leaves the rest of it with the
  # walk, and the next walk or read starts at the next batch. An Enumerator kept and advanced with
  # #next is the one walk, and goes on where it was.
  describe "a walk left part-way through a batch" do
    sheet_rows = lambda do
      HyperTabular::Workbook.open(File.join(workbooks, "generated-a.xlsx"))
                            .sheet(0, HyperTabular::SheetOptions.new(batch_rows: 3), [column.text(0), column.text(1)])
    end
    sources = {
      "reader" => [-> { reader.new(text, dialect_class::CSV, plan, batch_rows: 4) }, 4],
      "sheet" => [sheet_rows, 3]
    }
    sources.each do |name, (make, size)|
      it "leaves the rest of that batch behind, as Rust and Python do, #{name}" do
        lines = make.call.each.map(&:line)
        left = {
          first: ->(held) { held.first(1) },
          break: ->(held) { held.each { |_row| break } },
          raise: lambda do |held|
            held.each { |_row| raise Interrupt }
          rescue Interrupt
            nil
          end
        }
        left.each do |how, leave|
          held = make.call
          leave.call(held)
          expect([how, held.each.map(&:line)]).to eq([how, lines.drop(size)])
        end
      end

      it "goes on where it was through an Enumerator kept and advanced, #{name}" do
        lines = make.call.each.map(&:line)
        walk = make.call.each
        expect((1..lines.size).map { walk.next.line }).to eq(lines)
        expect { walk.next }.to raise_error(StopIteration)
      end
    end
  end

  describe "an exception that lands in a read blocked on a pipe" do
    stalled = lambda do
      r, w = IO.pipe
      w.write("a,b\n0,r0\n1,r1\n")
      [r, w, reader.new(r, dialect_class::CSV, plan, batch_rows: 2)]
    end
    # Writes the rest, +after+ seconds on, and closes the pipe, so that nothing waits for ever.
    finish = ->(w, after) { Thread.new { sleep after; w.write("2,r2\n3,r3\n") rescue nil; w.close rescue nil } }

    it "refuses every later read, naming what landed, since the read may have lost bytes" do
      r, w, held = stalled.call
      expect(held.read.values(0)).to eq([0, 1])
      main = Thread.current
      writer = finish.call(w, 2)
      Thread.new do
        Thread.pass until main.status == "sleep"
        main.raise(Interrupt, "in the pipe")
      end
      expect { held.read }.to raise_error(Interrupt, "in the pipe")
      refusal = /\Athe reader can't read on: Interrupt \(in the pipe\) was raised while it was reading its IO/
      expect { held.read }.to raise_error(RuntimeError, refusal)
      expect { held.each { nil } }.to raise_error(RuntimeError, refusal)
      writer.join
      r.close
    end

    it "is broken by a Timeout while it waits, not held until the IO answers" do
      r, w, held = stalled.call
      expect(held.read.values(0)).to eq([0, 1])
      writer = finish.call(w, 3)
      started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      expect { Timeout.timeout(0.3) { held.read } }.to raise_error(Timeout::Error)
      expect(Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).to be < 1.5
      expect { held.read }.to raise_error(RuntimeError, /was raised while it was reading its IO/)
      writer.join
      r.close
    end

    # Ruby can't tell an IO's own error from one that landed in its read (Timeout.timeout's class
    # can be Errno::ETIMEDOUT), so nothing the IO raises is taken as resumable.
    [IOError, Errno::ETIMEDOUT].each do |kind|
      it "makes the IO's own #{kind} final" do
        says = kind.new("the wire went").message
        raising = Class.new do
          def initialize(kind) = (@kind = kind; @parts = ["a,b\n0,r0\n", :raise, "1,r1\n"])

          def readpartial(_length)
            part = @parts.shift
            raise EOFError if part.nil?
            raise @kind, "the wire went" if part == :raise

            part
          end
        end
        held = reader.new(raising.new(kind), dialect_class::CSV, plan, batch_rows: 1)
        expect(held.read.values(0)).to eq([0])
        expect { held.read }.to raise_error(kind, says)
        refusal = "the reader can't read on: #{kind} (#{says}) was raised while it was reading its IO, " \
                  "and what that read had taken from the IO may be lost"
        2.times { expect { held.read }.to raise_error(RuntimeError, refusal) }
        expect { held.each { nil } }.to raise_error(RuntimeError, refusal)
      end
    end
  end

  it "hands a broken sheet's intact rows over before its failure, with an exception as they are made" do
    book = HyperTabular::Workbook.open(File.join(workbooks, "broken.xlsx"))
    walk = lambda do
      sheet = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 1000, has_header: false), [column.text(0)])
      rows = []
      caught = 0
      failure = nil
      loop do
        batch = begin
          sheet.read
        rescue Interrupt
          caught += 1
          next
        rescue HyperTabular::TabularError => e
          failure = e.message
          break
        end
        break if batch.nil?

        rows.concat((0...batch.rows).map { |row| [batch.line(row), batch.raw(0, row)] })
      end
      [rows, failure, caught]
    end
    expected, failure, = walk.call
    expect(expected.size).to eq(2)
    armed = true
    allow(HyperTabular::Batch).to receive(:new).and_wrap_original do |original, *args, **options|
      delivered.call(Interrupt.new("as the rows were made")) if armed
      armed = false
      original.call(*args, **options)
    end
    expect(walk.call).to eq([expected, failure, 1])
  end

  describe "an exception delivered as a batch is made" do
    encodings.each do |encoding, codec|
      { string: ->(bytes) { bytes }, io: ->(bytes) { pieces.new(bytes, 13) } }.each do |source, made|
        it "hands that batch over on the next read, numbered the same, #{encoding} #{source}" do
          made_count = 0
          allow(HyperTabular::Batch).to receive(:new).and_wrap_original do |original, *args, **options|
            made_count += 1
            original.call(*args, **options)
          end
          drained.call(reader.new(made.call(text.encode(codec)), dialect_of.call(encoding), plan, batch_rows: 4))
          expect(made_count).to be > 4
          (1..made_count).each do |armed_at|
            batches = 0
            allow(HyperTabular::Batch).to receive(:new).and_wrap_original do |original, *args, **options|
              batches += 1
              delivered.call(Interrupt.new("delivered")) if batches == armed_at
              original.call(*args, **options)
            end
            held = reader.new(made.call(text.encode(codec)), dialect_of.call(encoding), plan, batch_rows: 4)
            out, caught = drained.call(held)
            expect([armed_at, out]).to eq([armed_at, control])
            expect(caught).to eq(1)
          end
        end
      end
    end

    it "hands over a batch to a caller that holds the exception back itself" do
      armed = true
      allow(HyperTabular::Batch).to receive(:new).and_wrap_original do |original, *args, **options|
        delivered.call(Interrupt.new("held back")) if armed
        armed = false
        original.call(*args, **options)
      end
      held = reader.new(text, dialect_class::CSV, plan, batch_rows: 4)
      first = nil
      caught = begin
        Thread.handle_interrupt(Interrupt => :never) do
          first = held.read
          Thread.handle_interrupt(Interrupt => :immediate) { sleep 1 }
        end
        false
      rescue Interrupt
        true
      end
      expect(caught).to be(true)
      out, = drained.call(held)
      expect(rows_of.call(first) + out).to eq(control)
    end

    it "hands a sheet's batch over on the next read" do
      book = HyperTabular::Workbook.open(File.join(workbooks, "generated-a.xlsx"))
      read_sheet = lambda do
        sheet = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 3), [column.text(0), column.text(1)])
        out = []
        caught = 0
        loop do
          batch = begin
            sheet.read
          rescue Interrupt
            caught += 1
            next
          end
          break if batch.nil?

          out.concat(rows_of.call(batch))
        end
        [out, caught]
      end
      made_count = 0
      allow(HyperTabular::Batch).to receive(:new).and_wrap_original do |original, *args, **options|
        made_count += 1
        original.call(*args, **options)
      end
      expected, = read_sheet.call
      expect(made_count).to be > 1
      (1..made_count).each do |armed_at|
        batches = 0
        allow(HyperTabular::Batch).to receive(:new).and_wrap_original do |original, *args, **options|
          batches += 1
          delivered.call(Interrupt.new("delivered")) if batches == armed_at
          original.call(*args, **options)
        end
        expect(read_sheet.call).to eq([expected, 1])
      end
    end
  end

  describe "an exception raised as a batch is made, after the core has read on" do
    it "refuses every later read of a reader, saying why" do
      (1..5).each do |armed_at|
        batches = 0
        allow(HyperTabular::Batch).to receive(:new).and_wrap_original do |original, *args, **options|
          batches += 1
          raise NoMemoryError, "no room" if batches == armed_at

          original.call(*args, **options)
        end
        held = reader.new(text, dialect_class::CSV, plan, batch_rows: 4)
        out = []
        (armed_at - 1).times { out.concat(rows_of.call(held.read)) }
        expect { held.read }.to raise_error(NoMemoryError, "no room")
        2.times do
          expect { held.read }
            .to raise_error(RuntimeError, /\Athe reader can't read on: NoMemoryError \(no room\) was raised/)
        end
        expect(out).to eq(control.first(4 * (armed_at - 1)))
      end
    end

    it "refuses every later read of a sheet, saying why" do
      book = HyperTabular::Workbook.open(File.join(workbooks, "generated-a.xlsx"))
      allow(HyperTabular::Batch).to receive(:new).and_raise(NoMemoryError, "no room")
      sheet = book.sheet(0, HyperTabular::SheetOptions.new(batch_rows: 3), [column.text(0)])
      expect { sheet.read }.to raise_error(NoMemoryError, "no room")
      expect { sheet.read }.to raise_error(RuntimeError, /\Athe sheet can't read on: NoMemoryError \(no room\)/)
    end
  end

  # The core reads the String last attached, by address on the Fiddle backend: the String it reads is
  # never changed, and the one after it is attached before anything else moves, so whatever lands on
  # the way leaves the core reading memory the reader still holds.
  describe "an exception as a grown buffer is attached" do
    def attaching(&wrap)
      if HyperTabular::BACKEND == :fiddle
        allow(HyperTabular::Runtime).to receive(:pin).and_wrap_original { |original, string| wrap.call { original.call(string) } }
      else
        allow_any_instance_of(HyperTabular::Runtime::Native::Delimited)
          .to receive(:attach).and_wrap_original { |original, string| wrap.call { original.call(string) } }
      end
    end

    encodings.each do |encoding, codec|
      { "delivered" => ->(error) { delivered.call(error) }, "raised" => ->(error) { raise error } }
        .each do |how, land|
        # Short records leave the unfinished one behind the start of the buffer; long ones, longer than
        # the buffer, are what makes a refill grow the String the core was last handed.
        { "short" => [text, control], "long" => [long_text, long_control] }.each do |length, (input, expected)|
          it "reads every row once, with its own line, when one is #{how} there, #{length} records, #{encoding}" do
            (1..16).each do |armed_at|
              attaches = nil # counted once the reader is made
              attaching do |&attach|
                attaches += 1 if attaches
                land.call(Interrupt.new("at attach")) if attaches == armed_at
                attach.call
              end
              source = pieces.new(input.encode(codec), 13)
              held = reader.new(source, dialect_of.call(encoding), plan, batch_rows: 4, buffer_bytes: 16)
              attaches = 0
              out = []
              loop do
                batch = begin
                  held.read
                rescue Interrupt
                  GC.compact if GC.respond_to?(:compact)
                  Array.new(2000) { "Z" * 64 }
                  next
                end
                break if batch.nil?

                out.concat(rows_of.call(batch))
              end
              expect([armed_at, out]).to eq([armed_at, expected])
            end
          end
        end
      end
    end
  end
end
