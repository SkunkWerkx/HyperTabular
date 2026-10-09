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
  # A name matches exactly — byte for byte, case and spaces included — and a name the header
  # has twice is found where it is first.
  class Header < Array
    # The ordinal of the first column named +name+ (a String). A name the header does not
    # have is a KeyError that says which — or, with a block, the block's value, the block
    # handed the name, as Hash#fetch does:
    #
    #   header.ordinal("id")                  # => 0
    #   header.ordinal("missing")             # KeyError: the header has no column named "missing"
    #   header.ordinal("missing") { nil }     # => nil
    #
    # A binary String is matched as the bytes it holds; a String in another encoding is
    # matched as its UTF-8 transcoding. TypeError for anything that is not a String.
    def ordinal(name)
      found = find_ordinal(name)
      return found unless found.nil?
      return yield(name) if block_given?

      raise KeyError.new("the header has no column named #{name.inspect}", receiver: self, key: name)
    end

    # The ordinal of the first column named +name+, or nil when the header has none: #ordinal
    # without the KeyError, matched the same way.
    def find_ordinal(name)
      raise TypeError, "a column name is a String; got #{name.class}" unless name.is_a?(String)

      wanted =
        case name.encoding
        when Encoding::UTF_8 then name
        when Encoding::BINARY then name.dup.force_encoding(Encoding::UTF_8)
        else
          begin
            name.encode(Encoding::UTF_8)
          rescue EncodingError
            return nil
          end
        end
      index(wanted)
    end
  end
end
