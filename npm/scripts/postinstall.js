const fs = require("fs");
const https = require("https");
const path = require("path");
const { execFileSync } = require("child_process");

const REPO = process.env.AGENTLINK_REPO || "loyality7/AgentLink";
const BIN_DIR = path.join(__dirname, "..", "bin");
const BIN_NAME = process.platform === "win32" ? "agentlink.exe" : "agentlink";
const DEST = path.join(BIN_DIR, BIN_NAME);

function getAssetName() {
  const p = process.platform;
  const a = process.arch;
  if (p === "win32" && a === "x64") return "agentlink-x86_64-pc-windows-msvc.zip";
  if (p === "darwin" && a === "arm64") return "agentlink-aarch64-apple-darwin.tar.gz";
  if (p === "darwin" && a === "x64") return "agentlink-x86_64-apple-darwin.tar.gz";
  if (p === "linux" && a === "x64") return "agentlink-x86_64-unknown-linux-gnu.tar.gz";
  if (p === "linux" && a === "arm64") return "agentlink-aarch64-unknown-linux-gnu.tar.gz";
  throw new Error(`Unsupported platform: ${p} ${a}`);
}

function download(url, outFile) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { "User-Agent": "agentlink-npm" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return download(res.headers.location, outFile).then(resolve, reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode} downloading ${url}`));
        }
        const stream = fs.createWriteStream(outFile);
        res.pipe(stream);
        stream.on("finish", () => stream.close(resolve));
        stream.on("error", reject);
      })
      .on("error", reject);
  });
}

async function main() {
  if (fs.existsSync(DEST)) return;
  fs.mkdirSync(BIN_DIR, { recursive: true });
  const asset = getAssetName();
  const url = `https://github.com/${REPO}/releases/latest/download/${asset}`;
  const tmp = path.join(BIN_DIR, asset);

  console.log(`Downloading ${asset}...`);
  await download(url, tmp);

  if (asset.endsWith(".zip")) {
    execFileSync(
      "powershell",
      ["-NoProfile", "-Command", `Expand-Archive -Path '${tmp}' -DestinationPath '${BIN_DIR}' -Force`],
      { stdio: "inherit" }
    );
  } else {
    execFileSync("tar", ["-xzf", tmp, "-C", BIN_DIR], { stdio: "inherit" });
    fs.chmodSync(DEST, 0o755);
  }
  fs.unlinkSync(tmp);
  console.log(`Installed ${DEST}`);
}

main().catch((err) => {
  console.error("Failed to install agentlink binary:", err.message);
  process.exit(1);
});

