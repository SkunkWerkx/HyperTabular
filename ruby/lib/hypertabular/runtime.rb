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
      # (state, input, input_len, last, specs, columns, column_count, max_rows, cells,
      #  cells_cap, arena, arena_cap, out)
      hypertabular_delimited_fill:
        [%i[pointer pointer size uint32 pointer pointer size size pointer size pointer size pointer], :int32],
      # (cell, len, out, cap)
      hypertabular_delimited_unescape: [%i[pointer size pointer size], :size]
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

      private

      # The shared library to dlopen: this install's native/{rid}/{lib}, or — the
      # development loop — the in-repo cargo build, exactly what the other bindings' local
      # staging does. Nil when neither exists.
      def library_path
        rid, lib_name = NativePlatform.rid_and_library_name
        path = File.join(NATIVE_DIR, rid, lib_name)
        return path if File.exist?(path)

        repo_build = File.expand_path(File.join(__dir__, "../../../rust/target/release", lib_name))
        File.exist?(repo_build) ? repo_build : nil
      end

      # Why there was nothing to load.
      def missing_library_message
        rid, lib_name = NativePlatform.rid_and_library_name
        "hypertabular: #{File.join(NATIVE_DIR, rid, lib_name)} not found (unsupported platform, " \
          "or this gem was built without a native library for it)"
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
          [name, Fiddle::Function.new(handle[name.to_s], arguments.map { |type| types.fetch(type) }, types.fetch(result))]
        end
      end
    end
  end
end
