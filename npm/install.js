#!/usr/bin/env node
// Downloads the SafeHell binary matching this machine from the GitHub release
// whose tag corresponds to this package's version, and vendors it next to the
// launcher in bin/.
//
// The binaries are far too large to ship six copies of in one npm tarball, and
// splitting them across per-platform optional dependencies means six more
// packages to publish and keep in step. Fetching at install time keeps this
// package to a few kilobytes.
//
// Run `node install.js --selftest` to check the platform mapping without
// touching the network.

"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const crypto = require("crypto");
const { execFileSync } = require("child_process");

const REPO = "suiflex/SafeHell";

// node platform:arch -> [release asset stem, archive extension, binary name]
const TARGETS = {
  "darwin:arm64": ["safehell-macos-aarch64", "tar.gz", "safehell"],
  "darwin:x64": ["safehell-macos-x86_64", "tar.gz", "safehell"],
  "linux:x64": ["safehell-linux-x86_64", "tar.gz", "safehell"],
  "linux:arm64": ["safehell-linux-aarch64", "tar.gz", "safehell"],
  "win32:x64": ["safehell-windows-x86_64", "zip", "safehell.exe"],
  "win32:arm64": ["safehell-windows-aarch64", "zip", "safehell.exe"],
};

function resolveTarget(platform, arch) {
  const target = TARGETS[`${platform}:${arch}`];
  if (!target) {
    throw new Error(
      `SafeHell has no prebuilt binary for ${platform}/${arch}. ` +
        `Install it from source with: cargo install safehell`
    );
  }
  return { stem: target[0], ext: target[1], binary: target[2] };
}

// Follows redirects by hand: GitHub serves release assets from a signed
// object-storage URL, and adding a redirect-following HTTP client would be the
// only dependency this package has.
function download(url, hops = 5) {
  return new Promise((resolve, reject) => {
    if (hops < 0) {
      reject(new Error("too many redirects"));
      return;
    }
    https
      .get(url, { headers: { "user-agent": "safehell-npm-installer" } }, (response) => {
        const { statusCode, headers } = response;
        if (statusCode >= 300 && statusCode < 400 && headers.location) {
          response.resume();
          resolve(download(new URL(headers.location, url).toString(), hops - 1));
          return;
        }
        if (statusCode !== 200) {
          response.resume();
          reject(new Error(`GET ${url} failed with HTTP ${statusCode}`));
          return;
        }
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => resolve(Buffer.concat(chunks)));
        response.on("error", reject);
      })
      .on("error", reject);
  });
}

// The download is unpacked and then executed, so it is verified first. The
// shell installer refuses to install without SHA256SUMS and this must not be
// the weaker path to the same binary.
function verify(sums, asset, archive) {
  const line = sums
    .split("\n")
    .find((entry) => entry.trim().endsWith(` ${asset}`) || entry.trim().endsWith(`*${asset}`));
  if (!line) {
    throw new Error(`SHA256SUMS has no entry for ${asset}`);
  }
  const expected = line.trim().split(/\s+/)[0];
  const actual = crypto.createHash("sha256").update(archive).digest("hex");
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${asset} (expected ${expected}, got ${actual})`);
  }
}

async function main() {
  const { version } = require("./package.json");
  const { stem, ext, binary } = resolveTarget(process.platform, process.arch);
  const asset = `${stem}.${ext}`;
  const base = `https://github.com/${REPO}/releases/download/v${version}`;

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "safehell-"));
  try {
    process.stderr.write(`Downloading ${asset} v${version}\n`);
    const [archive, sums] = await Promise.all([
      download(`${base}/${asset}`),
      download(`${base}/SHA256SUMS`),
    ]);
    verify(sums.toString("utf8"), asset, archive);

    const archivePath = path.join(scratch, asset);
    fs.writeFileSync(archivePath, archive);
    // bsdtar reads zip as well as tar.gz, and ships with macOS, modern Windows,
    // and every Linux image these binaries target.
    execFileSync("tar", ["-xf", archivePath, "-C", scratch], { stdio: "inherit" });

    const vendor = path.join(__dirname, "bin");
    fs.mkdirSync(vendor, { recursive: true });
    const destination = path.join(vendor, binary);
    fs.copyFileSync(path.join(scratch, binary), destination);
    fs.chmodSync(destination, 0o755);
    process.stderr.write(`Installed safehell v${version}\n`);
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

function selftest() {
  const assert = require("assert");
  assert.deepStrictEqual(resolveTarget("darwin", "arm64"), {
    stem: "safehell-macos-aarch64",
    ext: "tar.gz",
    binary: "safehell",
  });
  assert.deepStrictEqual(resolveTarget("win32", "x64"), {
    stem: "safehell-windows-x86_64",
    ext: "zip",
    binary: "safehell.exe",
  });
  assert.throws(() => resolveTarget("sunos", "sparc"), /no prebuilt binary/);

  // Every mapped asset must be one the release workflow actually produces.
  const built = new Set([
    "safehell-linux-x86_64",
    "safehell-linux-aarch64",
    "safehell-macos-x86_64",
    "safehell-macos-aarch64",
    "safehell-windows-x86_64",
    "safehell-windows-aarch64",
  ]);
  for (const [stem] of Object.values(TARGETS)) {
    assert.ok(built.has(stem), `${stem} is not built by release-build.yml`);
  }

  const sums = "aa\ncafe" + "0".repeat(60) + "  safehell-linux-x86_64.tar.gz\n";
  const body = Buffer.from("x");
  assert.throws(() => verify(sums, "safehell-linux-x86_64.tar.gz", body), /checksum mismatch/);
  assert.throws(() => verify(sums, "safehell-macos-arm64.tar.gz", body), /no entry/);
  const good = crypto.createHash("sha256").update(body).digest("hex");
  verify(`${good}  safehell-linux-x86_64.tar.gz\n`, "safehell-linux-x86_64.tar.gz", body);
  verify(`${good} *safehell-linux-x86_64.tar.gz\n`, "safehell-linux-x86_64.tar.gz", body);

  console.log("selftest ok");
}

if (process.argv.includes("--selftest")) {
  selftest();
} else {
  main().catch((error) => {
    process.stderr.write(`safehell: ${error.message}\n`);
    process.exit(1);
  });
}
