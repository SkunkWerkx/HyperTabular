require "date"
require "hypercast"
require_relative "hypertabular/native_platform"
require_relative "hypertabular/runtime"
require_relative "hypertabular/runtime/delimited"

# Tabular parsing with a HyperCast verdict for every cell: delimited text — CSV, TSV, any
# single-byte ASCII separator — read a batch at a time into typed columns by one Rust core
# (libhypertabular), called through Fiddle.
#
#   plan = [HyperTabular::Column.i32(0), HyperTabular::Column.text(1)]
#   reader = HyperTabular::DelimitedReader.new("id,name\n1,alice\nx,bob\n", HyperTabular::Dialect::CSV, plan)
#   reader.header                 # => ["id", "name"]
#   reader.read                   # => true
#   reader.values(1)              # => ["alice", "bob"]
#   reader.verdict(0, 1)          # => #<data HyperCast::Fault reason=:malformed, offset=0, length=1>
#   reader.raw(0, 1)              # => "x"
#
# Nothing is sniffed: the Dialect states the separator, the quoting and the header, and the
# plan states each Column's door and, for numbers, its notation. HyperCast is the judge:
# the verdicts (HyperCast::Success, HyperCast::Fault), the notation (HyperCast::NumFormat),
# the exact decimal (HyperCast::Decimal) and the declared options are that gem's own types,
# and a cell means exactly what HyperCast's door would say of the same text. A bad value is
# a verdict; a broken file is a TabularError.
module HyperTabular
  # This gem's own version — kept in lockstep with hypertabular.gemspec and the core's
  # rust/Cargo.toml.
  VERSION = "0.1.0"

  class << self
    # Whether the native core loaded and exports the ABI this binding was built against.
    # Probed once (a native_version round trip: the cheapest call the core has), cached,
    # and never raises: a missing shared library and an unsupported platform both answer
    # false. A reader keeps its own behavior — building one without the core raises the
    # precise LoadError — this only answers the question quietly.
    def available?
      return @available unless @available.nil?

      @available = begin
        native_version.is_a?(String)
      rescue LoadError, StandardError
        false
      end
    end

    # The version of the native core actually loaded, as "major.minor.patch" — read from
    # the library itself, not from this gem, so a consumer can prove the core behind the
    # reader is the one this binding was built against.
    def native_version
      word = Runtime::Delimited.version
      "#{word >> 16}.#{(word >> 8) & 0xFF}.#{word & 0xFF}"
    end
  end
end

require_relative "hypertabular/dialect"
require_relative "hypertabular/column"
require_relative "hypertabular/tabular_error"
require_relative "hypertabular/delimited_reader"

# --- backend selection. There is one backend today: the native libhypertabular shared
# library called through Fiddle. Every native call and every byte of native memory is behind
# HyperTabular::Runtime::Delimited, so a compiled extension, when there is one, replaces that
# class and is chosen here, the way hypercast chooses its own.
#
# HYPERTABULAR_PURE forces Fiddle, as HYPERCAST_PURE does for hypercast: CI runs the whole
# suite through it. With nothing else to choose from it is accepted and changes nothing.
HyperTabular::BACKEND = :fiddle
