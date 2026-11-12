class Rone < Formula
  desc "Makes System One decision models pleasant to use from the command-line"
  homepage "https://github.com/oteissonniere/rone"
  version "__VERSION__"
  license "MIT"

  if Hardware::CPU.arm?
    url "https://github.com/oteissonniere/rone/releases/download/v#{version}/rone-v#{version}-aarch64-apple-darwin.tar.gz"
    sha256 "__SHA256_ARM__"
  else
    url "https://github.com/oteissonniere/rone/releases/download/v#{version}/rone-v#{version}-x86_64-apple-darwin.tar.gz"
    sha256 "__SHA256_X86__"
  end

  def install
    bin.install "rone"
    bin.install "presets"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rone --version")
  end
end
