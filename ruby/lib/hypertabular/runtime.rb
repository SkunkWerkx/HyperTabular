# Autoloaded, not required, as in hypercast: Fiddle loads the first time the backend runs,
# and every path there goes through `functions` first, so a missing library is reported as
# missing_library_message rather than as whatever touching Fiddle raises first.
autoload :Fiddle, "fiddle"

module HyperTabular
  # Fiddle plumbing for the native libhypertabular shared library — dlopen/dlsym plus raw
  # C-ABI calls, no runtime bridge. A gem's files are plain files on disk once installed, so
  # native/{rid}/{lib} is dlopen'ed directly — no extraction.
  module Runtime
    NATIVE_DIR = File.join(__dir__, "native")

    # Every export of the library (rust/src/kernel/exports.rs), with its C signature spelled
    # in Symbols — resolved to Fiddle types only in load_functions, so nothing here touches
    # Fiddle until the backend actually runs.
    EXPORTS = {
      hypertabular_version: [[], :uint32],
      hypertabular_delimited_state_size: [[], :size],
      # (state, dialect): 1.0's, which no read takes; a spec holds it to what it always read.
      hypertabular_delimited_init: [%i[pointer pointer], :int32],
      # (state, options), the RawDelimitedOptions every read starts from.
      hypertabular_delimited_init_with: [%i[pointer pointer], :int32],
      # (encoding, input, input_len, last, out, cap, progress)
      hypertabular_transcode: [%i[uint32 pointer size uint32 pointer size pointer], :int32],
      # (state, input, input_len, last, names, names_cap, arena, arena_cap, out)
      hypertabular_delimited_header:
        [%i[pointer pointer size uint32 pointer size pointer size pointer], :int32],
      # (state, input, input_len, last, specs, columns, column_count, max_rows, buffers, out):
      # 1.0's, which no read takes, reading every format Western; a spec holds it to that.
      hypertabular_delimited_fill:
        [%i[pointer pointer size uint32 pointer pointer size size pointer pointer], :int32],
      # (state, input, input_len, last, specs, columns, column_count, max_rows, buffers, out) —
      # buffers is the BuffersWith block, of which a delimited fill reads the arena, the cell
      # table, the tokens and the row widths.
      hypertabular_delimited_fill_with:
        [%i[pointer pointer size uint32 pointer pointer size size pointer pointer], :int32],
      # (columns, column_count, rows, row_widths, expected_width, long_rows, cursor, out, cap, written)
      hypertabular_faults:
        [%i[pointer size size pointer uint32 uint32 pointer pointer size pointer], :int32],
      # (input, input_len, arena, arena_len, cells, typed, rows, columns, formats, out)
      hypertabular_suggest:
        [%i[pointer size pointer size pointer pointer size size pointer pointer], :int32],
      # (cell, len, out, cap)
      hypertabular_delimited_unescape: [%i[pointer size pointer size], :size],
      hypertabular_workbook_state_size: [[], :size],
      # (state, container, container_len, buffers, out)
      hypertabular_workbook_open: [%i[pointer pointer size pointer pointer], :int32],
      hypertabular_workbook_sheets: [%i[pointer pointer size pointer pointer], :int32],
      hypertabular_workbook_strings: [%i[pointer pointer size pointer pointer], :int32],
      hypertabular_workbook_styles: [%i[pointer pointer size pointer pointer], :int32],
      # (state, container, container_len, part, part_len, index, has_header, skip_empty_rows, out):
      # 1.0's, which no read takes.
      hypertabular_workbook_sheet:
        [%i[pointer pointer size pointer size uint32 uint32 uint32 pointer], :int32],
      # (state, container, container_len, part, part_len, index, options, out)
      hypertabular_workbook_sheet_with:
        [%i[pointer pointer size pointer size uint32 pointer pointer], :int32],
      hypertabular_workbook_header: [%i[pointer pointer size pointer pointer], :int32],
      # (state, container, container_len, specs, columns, column_count, max_rows, buffers, out):
      # 1.0's, which no read takes.
      hypertabular_workbook_fill: [%i[pointer pointer size pointer pointer size size pointer pointer], :int32],
      # (state, container, container_len, specs, columns, column_count, max_rows, buffers, out) —
      # buffers is the BuffersWith block.
      hypertabular_workbook_fill_with:
        [%i[pointer pointer size pointer pointer size size pointer pointer], :int32],
      # (text, len, out)
      hypertabular_range_parse: [%i[pointer size pointer], :int32],
      hypertabular_batch_check: [%i[pointer size uint64 uint64 uint32 pointer], :int32]
    }.freeze

    @mutex = Mutex.new
    @functions = nil

    # The entry points the Magnus extension also defines, the Fiddle way. They live in a module
    # Runtime extends rather than on its singleton class, so the extension's
    # define_singleton_method puts its definition in front of each: an override through
    # the ancestor chain, not a redefinition, and `ruby -w` has nothing to say about it (1.0.0
    # printed "method redefined" for it, and for Delimited.start and .version, on every
    # require under -w). With the extension loaded no call ever reaches it.
    module FiddleBackend
      # A quoted delimited cell — the bytes a flagged cell-table entry names — with its
      # quotes resolved, as the core cast it.
      def unescape(quoted)
        input = pin(quoted)
        out = buffer([quoted.bytesize, 1].max)
        written = function(:hypertabular_delimited_unescape).call(input, quoted.bytesize, out, quoted.bytesize)
        out[0, written]
      end

      # The index of the first of +specs+ (Column.specs' form) the core refuses, or nil. A fill of
      # no rows judges a column's door, what it declares, its format, date options, vocabulary and
      # tokens, and writes nothing. It answers on a state of its own, so a sheet's plan is asked
      # the same way.
      def refused(specs)
        state = buffer(function(:hypertabular_delimited_state_size).call)
        options = buffer(Dialect::OPTIONS_BYTES)
        options[0, Dialect::OPTIONS_BYTES] = Dialect::CSV.packed_with
        raise Delimited::CONTRACT unless
          function(:hypertabular_delimited_init_with).call(state, options) == Delimited::OK

        value = buffer(32)
        verdict = buffer(Columns::VERDICT_BYTES)
        table = buffer(Columns::COLUMN_BUFFER_BYTES)
        table[0, Columns::COLUMN_BUFFER_BYTES] = Columns.column_buffer(value, verdict, nil)
        memory = buffer(Columns::BUFFERS_WITH_BYTES)
        memory[0, Columns::WORD] = Columns.word(Columns::BUFFERS_WITH_BYTES)
        out = buffer(Delimited::FILLED_BYTES)
        fill = function(:hypertabular_delimited_fill_with)
        specs.index do |spec|
          lists = []
          bytes = Columns.spec_with(spec, lists)
          held = buffer(bytes.bytesize)
          held[0, bytes.bytesize] = bytes
          fill.call(state, nil, 0, 0, held, table, 1, 0, memory, out) == Delimited::ERR_CONTRACT
        end
      end

      # The next step of a declared encoding's decode: +length+ bytes of +input+ from +start+, with
      # +last+ saying nothing follows them, into at most +capacity+ bytes of UTF-8. Answers
      # [consumed, written, failed, failure_at, out], +out+ a binary String of its own.
      def transcode(encoding, input, start, length, last, capacity)
        out = buffer(capacity)
        progress = buffer(TRANSCODED_BYTES)
        base = pin(input)
        raise Delimited::CONTRACT unless
          function(:hypertabular_transcode).call(encoding, base.to_i + start, length, last ? 1 : 0, out, capacity,
                                                 progress) == Delimited::OK

        consumed, written, failure_at, failed = progress[0, TRANSCODED_BYTES].unpack("Q<3L<")
        [consumed, written, failed, failure_at, out[0, written]]
      end

      # Every RawProblem of a batch, as one binary String: +verdicts+ is each column's array as the
      # core wrote it, +notes+ the same or nil, and +widths+ the row widths or nil.
      def faults(verdicts, notes, rows, widths, expected, long_rows)
        pinned = verdicts.map { |column| pin(column) }
        noted = notes&.map { |column| pin(column) }
        table = buffer([Columns::COLUMN_BUFFER_BYTES * verdicts.size, 1].max)
        verdicts.each_index do |column|
          table[Columns::COLUMN_BUFFER_BYTES * column, Columns::COLUMN_BUFFER_BYTES] =
            Columns.column_buffer(nil, pinned[column], noted && noted[column])
        end
        held = widths && pin(widths)
        cursor = buffer(8)
        written = buffer(Columns::WORD)
        out = buffer(FAULTS_BYTES)
        call = function(:hypertabular_faults)
        listed = "".b
        loop do
          code = call.call(table, verdicts.size, rows, held || 0, expected, long_rows, cursor, out,
                           FAULTS_BYTES / PROBLEM_BYTES, written)
          raise Delimited::CONTRACT unless code == Delimited::OK

          entries = written[0, Columns::WORD].unpack1("J")
          listed << out[0, PROBLEM_BYTES * entries]
          return listed if entries < FAULTS_BYTES / PROBLEM_BYTES
        end
      end

      # What the core makes of a sample: door, param, status, matched and sampled for each of
      # +columns+, flat, or nil if it refuses. +input+ is the sample's bytes, +cells+ its spans a
      # column after another, +typed+ a byte for each or nil, +formats+ a packed format for each.
      def suggest(input, cells, typed, rows, columns, formats)
        out = buffer([SUGGESTION_BYTES * columns, 1].max)
        pins = [input, cells, typed, formats].map { |text| text && pin(text) }
        code = function(:hypertabular_suggest).call(pins[0], input.bytesize, nil, 0, pins[1], pins[2] || 0, rows,
                                                    columns, pins[3], out)
        code == Delimited::OK ? out[0, SUGGESTION_BYTES * columns].unpack("L<*") : nil
      end

      # The core's hypertabular_batch_check: [code, needed, limit, buffer_bytes, refused,
      # sample_rows, width]. The Magnus extension defines its own, as it does range_parse.
      def batch_check(specs, batch_rows, buffer_bytes, flags)
        lists = []
        packed = specs.map { |spec| Columns.spec_with(spec, lists) }.join
        held = buffer(packed.bytesize)
        held[0, packed.bytesize] = packed unless packed.empty?
        out = buffer(BATCH_CHECK_BYTES)
        code = function(:hypertabular_batch_check).call(held, specs.size, batch_rows, buffer_bytes, flags, out)
        needed, limit, start, sample_rows, width, refused = out[0, BATCH_CHECK_BYTES].unpack("Q<5L<")
        [code, needed, limit, start, refused, sample_rows, width]
      end

      # A range's four corners as the core reads them from +text+, or nil if it isn't one.
      def range_parse(text)
        out = buffer(RANGE_BYTES)
        return nil unless function(:hypertabular_range_parse).call(pin(text), text.bytesize, out) == Delimited::OK

        out[0, RANGE_BYTES].unpack("L<4")
      end
    end

    # Flags to hypertabular_batch_check (RawBatchCheck in the core).
    BATCH_NOTES = 1
    BATCH_WIDTHS = 2
    BATCH_SHEET = 4
    # RawBatchCheck's refused codes.
    BATCH_ROWS_ZERO = 1
    BUFFER_BYTES_ZERO = 2
    ROW_TOO_WIDE = 4
    # Bytes in one RawBatchCheck.
    BATCH_CHECK_BYTES = 48
    # Bytes in one RawTranscoded.
    TRANSCODED_BYTES = 32
    # Bytes in one RawProblem.
    PROBLEM_BYTES = 32
    # The room one hypertabular_faults call lists into.
    FAULTS_BYTES = PROBLEM_BYTES * 256
    # Bytes in one RawSuggestion.
    SUGGESTION_BYTES = 20
    # Bytes in one RawRange.
    RANGE_BYTES = 16
    # The allocator behind every buffer, which has to be the one RUBY_FREE frees: Ruby's own in
    # CRuby, and libc's in Fiddle's FFI backend, the one every other Ruby takes, whose RUBY_FREE is
    # libc's free.
    ZEROING = RUBY_ENGINE == "ruby" ? "ruby_xcalloc" : "calloc"

    extend FiddleBackend

    class << self
      # The export's Fiddle::Function, for the caller to invoke directly.
      def function(symbol)
        functions.fetch(symbol)
      end

      # Every native allocation goes through here, and loads the library first: that is what
      # keeps a missing library reported as missing_library_message instead of as whatever
      # touching Fiddle raises first. Zeroed, freed with the object that holds it.
      #
      # Many of these are sized by a caller, so nothing here may touch the memory: pages nobody
      # reads or writes stay the OS's. Fiddle::Pointer.malloc writes every byte. ruby_xcalloc asks
      # calloc, counts the bytes towards the next collection as Pointer.malloc does, pairs with
      # the RUBY_FREE it has always been freed with, and raises NoMemoryError for what it can't get.
      #
      # Fiddle::Pointer takes its size as a C long, 32 bits on 64-bit Windows. A size past that is
      # refused here, before the allocation it would otherwise strand.
      def buffer(size)
        functions
        raise NoMemoryError, "failed to allocate memory" if size >= 1 << ((Fiddle::SIZEOF_LONG * 8) - 1)

        # A zero-sized request still answers an address of its own. TruffleRuby and JRuby's
        # allocator is calloc, which answers null where ruby_xcalloc raises.
        address = zeroing.call(1, [size, 1].max).to_i
        raise NoMemoryError, "failed to allocate memory" if address.zero?

        Fiddle::Pointer.new(address, size, Fiddle::RUBY_FREE)
      end

      # Asks the core whether a read may ask for this much, before anything is allocated for it:
      # the size to start the input buffer at, or an ArgumentError in the core's terms. +specs+ is
      # Column.specs' answer, [] where no plan is known yet, and +flags+ is batch_flags' answer. What
      # is here is only what a count has to be for the core to be asked at all.
      def check_sizes(specs, batch_rows, buffer_bytes, flags)
        counted("batch_rows", batch_rows)
        counted("buffer_bytes", buffer_bytes)
        code, needed, limit, start, refused, _, width = batch_check(specs, batch_rows, buffer_bytes, flags)
        return start if code == Delimited::OK
        raise Delimited::CONTRACT unless code == Delimited::ERR_BATCH

        raise ArgumentError, case refused
                             when BATCH_ROWS_ZERO then "batch_rows must be at least 1"
                             when BUFFER_BYTES_ZERO then "buffer_bytes must be at least 1"
                             when ROW_TOO_WIDE
                               "a row of #{width} columns asks for #{needed} bytes, past the limit of #{limit}"
                             else "batch_rows asks for #{needed} bytes, past the limit of #{limit}"
                             end
      end

      # The rows of a sampler's batch for a sampler wanting +wanted+ rows: the core's answer, however
      # many batches that takes. Only a row past the limit is refused, in its words.
      def sample_rows(specs, wanted, flags)
        _, _, _, _, _, rows, = batch_check(specs, wanted, 1, flags)
        check_sizes(specs, wanted, 1, flags) if rows.zero?
        rows
      end

      # The flags batch_check takes, for a read that keeps +notes+ and +widths+.
      def batch_flags(notes, widths, sheet: false)
        (notes ? BATCH_NOTES : 0) | (widths ? BATCH_WIDTHS : 0) | (sheet ? BATCH_SHEET : 0)
      end

      private def counted(name, count)
        raise ArgumentError, "#{name} must be an Integer; got #{count.inspect}" unless count.is_a?(Integer)
        raise ArgumentError, "#{name} must not be negative; got #{count}" if count.negative?
        raise ArgumentError, "#{name} must fit in 64 bits; got #{count}" if count >= 1 << 64
      end

      # A pointer to a String's own bytes — nothing is copied. While the returned object is
      # alive the String is held where it is: Fiddle marks what it wraps as unmovable, so
      # the garbage collector's compaction cannot relocate an embedded String under a native
      # call. The address is good until the String is next modified.
      def pin(string)
        functions
        Fiddle::Pointer[string]
      end

      private

      # The allocator, resolved beside the exports, so a Ruby that doesn't have it is a LoadError
      # that available? answers for, not a failure at the first read. ruby_xcalloc needs the lock
      # Fiddle would otherwise let go of around a call.
      def zeroing
        functions
        @zeroing
      end

      def load_zeroing
        Fiddle::Function.new(Fiddle.dlopen(nil)[ZEROING], [Fiddle::TYPE_SIZE_T, Fiddle::TYPE_SIZE_T],
                             Fiddle::TYPE_VOIDP, need_gvl: true)
      rescue Fiddle::DLError => e
        raise LoadError, "HyperTabular's Fiddle backend can't find #{ZEROING} in this Ruby: #{e.message}"
      end

      # The shared library to dlopen: this install's native/{rid}/{lib}, or — the
      # development loop — the in-repo cargo build, exactly what the other bindings' local
      # staging does. Nil when neither exists.
      def library_path
        HyperCast::Interop.library_path("hypertabular", NATIVE_DIR, File.expand_path("../../..", __dir__))
      end

      # Why Fiddle found nothing to load. A precompiled platform gem is the one install where
      # that is by design rather than a gap: it carries only its Magnus extensions, and the
      # Fiddle backend is reached there only by forcing it (HYPERTABULAR_PURE) or because none
      # of its extensions loaded — a gem RubyGems matched to a Ruby it was not built for. So
      # that case names its fix, the universal gem, which carries every platform's library,
      # instead of a missing path that reads like a packaging bug — hypercast's wording for
      # the same case. Both arguments are parameters only so the specs can ask for every
      # wording.
      def missing_library_message(gem_platform = Gem.loaded_specs["hypertabular"]&.platform,
                                  forced = ENV.key?("HYPERTABULAR_PURE"))
        rid, lib_name = HyperCast::NativePlatform.rid_and_library_name(library: "hypertabular")
        missing = File.join(NATIVE_DIR, rid, lib_name)
        if gem_platform && gem_platform.to_s != Gem::Platform::RUBY
          reason =
            if forced
              "HYPERTABULAR_PURE forces the Fiddle backend (unset it to use the extension)"
            else
              "none of its extensions loads on this Ruby (#{RUBY_VERSION}, #{RUBY_PLATFORM})"
            end
          "hypertabular: this #{gem_platform} platform gem carries only Magnus extensions, no " \
            "Fiddle library, and #{reason}. The universal gem has the Fiddle backend for every " \
            "platform: `gem install hypertabular --platform ruby`, or Bundler's " \
            "force_ruby_platform (#{missing} not found)"
        else
          "hypertabular: #{missing} not found (unsupported platform, or this gem was built " \
            "without a native library for it)"
        end
      end

      # Loaded lazily and exactly once; the native library and its function pointers live
      # for the process's lifetime (never dlclose'd). The unsynchronized read is the fast
      # path; the benign race re-checks under the lock.
      def functions
        @functions || @mutex.synchronize { @functions ||= load_functions }
      end

      def load_functions
        path = library_path
        raise LoadError, missing_library_message if path.nil?

        handle = Fiddle.dlopen(path)
        @zeroing = load_zeroing
        types = {
          pointer: Fiddle::TYPE_VOIDP, size: Fiddle::TYPE_SIZE_T,
          uint32: Fiddle::TYPE_UINT32_T, int32: Fiddle::TYPE_INT32_T, uint64: Fiddle::TYPE_UINT64_T
        }
        EXPORTS.to_h do |name, (arguments, result)|
          [name, Fiddle::Function.new(handle[name.to_s], arguments.map { |type|
            types.fetch(type)
          }, types.fetch(result))]
        end
      end
    end
  end
end
