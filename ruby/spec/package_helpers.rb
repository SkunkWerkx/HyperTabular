require "zlib"

# A workbook package made here, for what no corpus package shows: a sheet of a given shape, and a
# part of a given deflated length.
module PackageHelpers
  MAIN = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'.freeze

  # A ZIP of +parts+ (name to bytes), each deflated.
  def self.zip(parts)
    body = "".b
    central = "".b
    parts.each do |name, data|
      raw = Zlib::Deflate.new(Zlib::DEFAULT_COMPRESSION, -Zlib::MAX_WBITS)
      packed = raw.deflate(data.b, Zlib::FINISH)
      raw.close
      crc = Zlib.crc32(data.b)
      shape = [20, 0, 8, 0, 0, crc, packed.bytesize, data.bytesize, name.bytesize, 0].pack("v5L<3v2")
      central << ["PK\x01\x02", 20].pack("a4v") << shape << [0, 0, 0, 0, body.bytesize].pack("v3L<2") << name
      body << "PK\x03\x04" << shape << name << packed
    end
    body + central + ["PK\x05\x06", 0, 0, parts.size, parts.size, central.bytesize, body.bytesize, 0].pack("a4v4L<2v")
  end

  # The smallest package a reader takes, with +rows+ as its one sheet's and +strings+, if given, as
  # its shared strings.
  def self.xlsx(rows, strings: nil)
    parts = {
      "[Content_Types].xml" => "<Types/>",
      "_rels/.rels" => '<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/' \
                       'officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>',
      "xl/workbook.xml" => %(<workbook #{MAIN} xmlns:r="r"><sheets><sheet name="Data" sheetId="1" r:id="rId0"/>) +
                           "</sheets></workbook>",
      "xl/_rels/workbook.xml.rels" => '<Relationships><Relationship Id="rId0" Type="http://x/worksheet" ' \
                                      'Target="worksheets/sheet0.xml"/></Relationships>',
      "xl/worksheets/sheet0.xml" => "<worksheet #{MAIN}><sheetData>#{rows}</sheetData></worksheet>"
    }
    parts["xl/sharedStrings.xml"] = "<sst #{MAIN}>#{strings}</sst>" if strings
    zip(parts)
  end
end
