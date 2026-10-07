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
      # (state, dialect)
      hypertabular_delimited_init: [%i[pointer pointer], :int32],
      # (state, input, input_len, last, names, names_cap, arena, arena_cap, out)
      hypertabular_delimited_header:
        [%i[pointer pointer size uint32 pointer size pointer size pointer], :int32],
      # (state, input, input_len, last, specs, columns, column_count, max_rows, buffers, out) —
      # buffers is the Buffers block the workbook calls take, of which a delimited fill reads
      # only the arena and the cell table.
      hypertabular_delimited_fill:
        [%i[pointer pointer size uint32 pointer pointer size size pointer pointer], :int32],
      # (cell, len, out, cap)
      hypertabular_delimited_unescape: [%i[pointer size pointer size], :size],
      hypertabular_workbook_state_size: [[], :size],
      # (state, container, container_len, buffers, out)
      hypertabular_workbook_open: [%i[pointer pointer size pointer pointer], :int32],
      hypertabular_workbook_sheets: [%i[pointer pointer size pointer pointer], :int32],
      hypertabular_workbook_strings: [%i[pointer pointer size pointer pointer], :int32],
      hypertabular_workbook_styles: [%i[pointer pointer size pointer pointer], :int32],
      # (state, container, container_len, part, part_len, index, has_header, skip_empty_rows, out)
      hypertabular_workbook_sheet:
        [%i[pointer pointer size pointer size uint32 uint32 uint32 pointer], :int32],
      hypertabular_workbook_header: [%i[pointer pointer size pointer pointer], :int32],
      # (state, container, container_len, specs, columns, column_count, max_rows, buffers, out)
      hypertabular_workbook_fill: [%i[pointer pointer size pointer pointer size size pointer pointer], :int32]
    }.freeze

    @mutex = Mutex.new
    @functions = nil

    class << self
      # The export's Fiddle::Function, for the caller to invoke directly.
      def function(symbol)
        functions.fetch(symbol)
      end

      # Every native allocation goes through here, and loads the library first: that is what
      # keeps a missing library reported as missing_library_message instead of as whatever
      # touching Fiddle raises first. Zeroed, freed with the object that holds it.
      def buffer(size)
        functions
        Fiddle::Pointer.malloc(size, Fiddle::RUBY_FREE)
      end

      # A pointer to a String's own bytes — nothing is copied. While the returned object is
      # alive the String is held where it is: Fiddle marks what it wraps as unmovable, so
      # the garbage collector's compaction cannot relocate an embedded String under a native
      # call. The address is good until the String is next modified.
      def pin(string)
        functions
        Fiddle::Pointer[string]
      end

      # A quoted delimited cell — the bytes a flagged cell-table entry names — with its
      # quotes resolved, as the core cast it.
      def unescape(quoted)
        input = pin(quoted)
        out = buffer([quoted.bytesize, 1].max)
        written = function(:hypertabular_delimited_unescape).call(input, quoted.bytesize, out, quoted.bytesize)
        out[0, written]
      end

      private

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
        types = {
          pointer: Fiddle::TYPE_VOIDP, size: Fiddle::TYPE_SIZE_T,
          uint32: Fiddle::TYPE_UINT32_T, int32: Fiddle::TYPE_INT32_T
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
