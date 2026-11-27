#!/usr/bin/env node
const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const binName = process.platform === "win32" ? "agentlink.exe" : "agentlink";
const binPath = path.join(__dirname, binName);

if (!fs.existsSync(binPath)) {
  console.error("agentlink binary not found. Re-run `npm install -g agentlink-cli` or run `node scripts/postinstall.js`.");
  process.exit(1);
}

const res = spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" });
process.exit(res.status ?? 1);

