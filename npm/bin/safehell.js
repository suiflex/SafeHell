#!/usr/bin/env node
// Launcher: hands every argument to the vendored binary and propagates its exit
// code, so `npx @suiflex/safehell ...` behaves like the real CLI.

"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const binary = path.join(__dirname, process.platform === "win32" ? "safehell.exe" : "safehell");

if (!fs.existsSync(binary)) {
  process.stderr.write(
    "safehell: the binary is missing. The postinstall download may have been " +
      "skipped or blocked.\nReinstall with: npm rebuild @suiflex/safehell\n"
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  process.stderr.write(`safehell: ${result.error.message}\n`);
  process.exit(1);
}
// A signalled child has a null status; report it the way a shell would.
process.exit(result.status === null ? 1 : result.status);
