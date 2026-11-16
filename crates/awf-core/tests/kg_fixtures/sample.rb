require "json"
require_relative "util"

module Acme
  class Sample < Base
    def run
      step
      Util.assist(name)
      helper(1)
    end

    def step; end

    def self.build
      new
    end
  end
end

def helper(n)
  JSON.dump(n)
end
