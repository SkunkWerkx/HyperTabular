Gem::Specification.new do |spec|
  spec.name = "hypertabular"
  # Kept in lockstep with HyperTabular::VERSION (lib/hypertabular.rb) and rust/Cargo.toml.
  spec.version = "0.8.0"
  spec.summary = "Delimited text and workbooks read a batch at a time into typed columns, " \
                 "a HyperCast verdict for every cell"
  spec.description = <<~DESC
    CSV, TSV and any single-byte ASCII separator, and XLSX and ODS workbooks, read by a
    native Rust core into typed column batches. The core owns no memory: this gem allocates the buffers once and the
    core fills them in one native call per batch. Every cell is a HyperCast verdict — the
    value, or a reason plus the offending span — and a file that is not rows of cells at
    all is an exception raised after the intact rows. Two backends behind one surface,
    selected automatically and both shipped prebuilt: a Magnus extension where a precompiled
    platform gem matches, and stdlib Fiddle everywhere else. No compile on install.
  DESC
  spec.authors = ["Brian Buvinghausen"]
  spec.license = "MIT"
  spec.homepage = "https://github.com/SkunkWerkx/HyperTabular"
  # The same floor as hypercast, whose Data case types this gem hands out.
  spec.required_ruby_version = ">= 3.3"

  # LICENSE is a local copy of the repo root's, not a reference to it: RubyGems stores a
  # "../LICENSE" entry with the `..` intact, a path-traversal entry no installer accepts.
  #
  # native/*/* is exactly the staged binaries, one directory per RID.
  spec.files = Dir["lib/**/*.rb"] + Dir["lib/hypertabular/native/*/*"] + ["README.md", "LICENSE"]
  spec.require_paths = ["lib"]

  # HyperCast is the judge: the verdict, fault, number-format and decimal types and the
  # declared-option tables are its own, taken from its gem rather than copied here.
  spec.add_dependency "hypercast", "~> 0.8.0"
  # A default gem through Ruby 3.x and a bundled one from 4.0, so it is declared. This
  # gemspec is the universal gem's, the one that runs on Fiddle; the precompiled platform
  # gems and the hypertabular-wasm gem drop it (Rakefile), since they carry no library for
  # Fiddle to open.
  spec.add_dependency "fiddle"
  # The test gems live in the Gemfile.
  spec.add_development_dependency "rake", "~> 13.0"
  spec.add_development_dependency "yard", "~> 0.9"

  spec.metadata["source_code_uri"] = spec.homepage
  spec.metadata["bug_tracker_uri"] = "#{spec.homepage}/issues"
  spec.metadata["changelog_uri"] = "#{spec.homepage}/blob/master/CHANGELOG.md"
  spec.metadata["documentation_uri"] = "#{spec.homepage}/tree/master/ruby#readme"
  # Pushing or yanking a version takes an account with multi-factor authentication on.
  spec.metadata["rubygems_mfa_required"] = "true"
end
