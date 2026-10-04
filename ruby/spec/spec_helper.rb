# HyperCast's gem is this gem's one runtime dependency that is not Ruby's own, and under
# Bundler it is simply there. The forge's Alpine suite (musl-suite/suite-musl.sh) runs
# without Bundler, though — it deletes the Gemfile, installs rspec and runs it, because
# Bundler would compile fiddle on the Rubies where it is a default gem — so there the
# dependency is installed here, at the requirement the gemspec declares and the way a
# consumer's `gem install hypertabular` would resolve it.
begin
  require "hypercast"
rescue LoadError
  raise if defined?(Bundler)

  gemspec = Gem::Specification.load(File.expand_path("../hypertabular.gemspec", __dir__))
  hypercast = gemspec.runtime_dependencies.find { |dependency| dependency.name == "hypercast" }
  # Activated by hand: this process has already looked for the gem once and remembers
  # that it was not there.
  Gem.install(hypercast.name, hypercast.requirement).each(&:activate)
  require "hypercast"
end

require_relative "../lib/hypertabular"

RSpec.configure do |config|
  config.expect_with :rspec do |expectations|
    expectations.include_chain_clauses_in_custom_matcher_descriptions = true
  end
  config.mock_with :rspec do |mocks|
    mocks.verify_partial_doubles = true
  end
  config.shared_context_metadata_behavior = :apply_to_host_groups
end
