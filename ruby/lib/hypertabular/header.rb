module HyperTabular
  # A header's names, in column order: what DelimitedReader#header and Sheet#header return.
  # It is the frozen Array of frozen UTF-8 Strings those have always returned — it equals
  # the Array of the same names, indexes, iterates and pattern-matches as one — and it can
  # also say where a name is, which is what a plan built from names reads with:
  #
  #   reader = HyperTabular::DelimitedReader.open("orders.csv", HyperTabular::Dialect::CSV)
  #   header = reader.header                  # => ["id", "name", "score"]
  #   reader.bind([HyperTabular::Column.text(header.ordinal("name")),
  #                HyperTabular::Column.f64(header.ordinal("score"))])
  #
  # A name matches exactly — byte for byte, case and spaces included. A name the header has twice
  # is refused by #ordinal, because a plan built from names mustn't silently read the wrong
  # column, and found where it is first by #find_ordinal.
  class Header < Array
    # The ordinal of the column named +name+ (a String). A name the header does not have is a
    # KeyError that says which — or, with a block, the block's value, the block handed the name,
    # as Hash#fetch does. A name two columns share is an AmbiguousColumn, block or no block: the
    # block answers a missing name, and a duplicate isn't one.
    #
    #   header.ordinal("id")                  # => 0
    #   header.ordinal("missing")             # KeyError: the header has no column named "missing"
    #   header.ordinal("missing") { nil }     # => nil
    #   HyperTabular::Header["id", "id"].ordinal("id")
    #   # AmbiguousColumn: the header names two columns "id", at positions 0 and 1: say which by position
    #
    # A binary String is matched as the bytes it holds; a String in another encoding is
    # matched as its UTF-8 transcoding. TypeError for anything that is not a String.
    def ordinal(name)
      wanted = wanted(name)
      found = wanted && index(wanted)
      if found.nil?
        return yield(name) if block_given?

        raise KeyError.new("the header has no column named #{name.inspect}", receiver: self, key: name)
      end
      (found + 1...size).each { |at| raise AmbiguousColumn.new(wanted, found, at) if self[at] == wanted }
      found
    end

    # The ordinal of the first column named +name+, or nil when the header has none: the optional
    # lookup, matched as #ordinal matches, which keeps the first of a name two columns share.
    def find_ordinal(name)
      wanted = wanted(name)
      wanted && index(wanted)
    end

    private

    # +name+ as the UTF-8 the names are in, or nil if it has no such form.
    def wanted(name)
      raise TypeError, "a column name is a String; got #{name.class}" unless name.is_a?(String)

      case name.encoding
      when Encoding::UTF_8 then name
      when Encoding::BINARY then name.dup.force_encoding(Encoding::UTF_8)
      else
        begin
          name.encode(Encoding::UTF_8)
        rescue EncodingError
          nil
        end
      end
    end
  end

  # A column name the header gives to more than one column, asked for by Header#ordinal: which
  # column it means is the caller's to say, by position. An ArgumentError, so that a rescue of the
  # KeyError a missing name raises doesn't take it for one.
  class AmbiguousColumn < ArgumentError
    # The name.
    attr_reader :name

    # The zero-based position of the first column so named, counted from a range's first column
    # on a ranged sheet, as a plan's ordinals are.
    attr_reader :first

    # The zero-based position of the second column so named.
    attr_reader :second

    # Raised by Header#ordinal; nothing else builds one.
    def initialize(name, first, second)
      @name = name
      @first = first
      @second = second
      super("the header names two columns #{name.inspect}, at positions #{first} and #{second}: say which by position")
    end
  end
end
