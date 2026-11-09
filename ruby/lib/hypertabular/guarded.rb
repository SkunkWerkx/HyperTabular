module HyperTabular
  # What a DelimitedReader and a Sheet share to stay whole while the caller's code runs in the middle
  # of a call (an IO's read, a plan's #each) or an exception lands in one (Timeout.timeout,
  # Thread#raise, Ctrl-C's Interrupt):
  #
  # - One call at a time. A call that the caller's code makes back into the same object, from inside
  #   one of its calls, is refused as busy, and the call it came from goes on as it was.
  # - What the core did and what the object says of it move together. Asynchronous exceptions wait
  #   while they move (Thread.handle_interrupt), and land after. The caller's IO is read outside that
  #   wait, so a Timeout still breaks a read that blocks, and leaves the reader refusing every later
  #   read, since that read may have lost what it had taken from the IO.
  # - A batch that is made is held until it is handed over. An exception that was waiting lands with
  #   the batch still held, and the next read hands it over, numbered the same; so does one that
  #   lands as the read returns it, anywhere short of the caller's own code. An exception raised
  #   inside the wait once the core has moved on, which nothing can undo, leaves the object refusing
  #   every later read, saying why.
  module Guarded
    private

    # What #hand_over's wait answers when an exception is waiting to land.
    NOT_YET = Object.new.freeze
    private_constant :NOT_YET

    # Runs the block as the one call into this object. A call it makes back in raises RuntimeError,
    # naming the object busy; nothing it leaves behind is touched.
    def exclusive
      raise "the #{noun} is in the middle of a call; nothing that call runs can call back into it" if @busy

      begin
        @busy = true
        yield
      ensure
        Thread.handle_interrupt(Object => :never) { @busy = false }
      end
    end

    # Runs the block with every asynchronous exception held until it is done.
    def atomically(&)
      Thread.handle_interrupt(Object => :never, &)
    end

    # Runs the block with every asynchronous exception held except while it blocks, as on a read of a
    # pipe that has nothing yet: one lands there, and nowhere else until the block is done.
    def while_blocked(&)
      Thread.handle_interrupt(Object => :on_blocking, &)
    end

    # Runs the block as a read, the one call into this object (#exclusive): if an exception unwinds
    # it after #hand_over has let go of a batch, that batch is held again for the next read, since it
    # never reached the caller.
    def handing_out
      owned = false
      exclusive do
        # In this order: the batch the last read handed out did reach its caller.
        @in_flight = nil
        owned = true
        yield
      end
    rescue Exception
      Thread.handle_interrupt(Object => :never) { give_back if owned }
      raise
    end

    # Yields every remaining batch, as #each and its kin do. An exception that lands between a read
    # and the yield gives that batch back for the next read; one from the block, which has had it,
    # gives nothing back.
    def batches
      between = false
      loop do
        between = true
        batch = read
        between = false
        break if batch.nil?

        yield batch
      end
      self
    rescue Exception
      Thread.handle_interrupt(Object => :never) { give_back if between }
      raise
    end

    # The batch held for the caller, let go of. An asynchronous exception waiting to land lands first,
    # with the batch still held for the next read; one the caller holds back itself lands in the
    # caller's own code, after this.
    def hand_over
      handed = atomically { Thread.pending_interrupt? ? NOT_YET : let_go }
      return handed unless handed.equal?(NOT_YET)

      atomically { let_go }
    end

    # The held batch, let go of and kept in flight until the read that hands it out is over.
    def let_go
      batch = @handing
      @handing = nil
      @in_flight = batch
    end

    # The batch in flight, held again.
    def give_back
      atomically do
        @handing ||= @in_flight
        @in_flight = nil
      end
    end

    # The failure that stands for +error+, raised inside the wait once the core had moved on: what
    # the core read is lost, so nothing after it can be read right.
    def spoiled(error)
      RuntimeError.new("the #{noun} can't read on: #{error.class} (#{error.message}) was raised after the core " \
                       "had read past a batch, and that batch is lost")
    end
  end
  private_constant :Guarded
end
