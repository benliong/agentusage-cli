class Agentusage < Formula
  desc "AI provider usage monitor — agent skill for model selection"
  homepage "https://agentusage.dev"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/ForgeRelayAI/agentusage-cli/releases/download/v#{version}/au-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_ARM64"
    else
      url "https://github.com/ForgeRelayAI/agentusage-cli/releases/download/v#{version}/au-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_X86"
    end
  end

  def install
    bin.install "au"
    (share/"agentusage/bundled_plugins").install Dir["bundled_plugins_pkg/*"]
  end

  test do
    system "#{bin}/au", "--version"
  end
end
