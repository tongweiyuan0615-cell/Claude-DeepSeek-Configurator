import { readFileSync } from "node:fs";

const files = [
  {
    name: "windows/package.json",
    read: () => JSON.parse(readFileSync("windows/package.json", "utf8")).version,
  },
  {
    name: "windows/package-lock.json",
    read: () => JSON.parse(readFileSync("windows/package-lock.json", "utf8")).version,
  },
  {
    name: "windows/package-lock.json packages[\"\"]",
    read: () => JSON.parse(readFileSync("windows/package-lock.json", "utf8")).packages[""].version,
  },
  {
    name: "windows/src-tauri/tauri.conf.json",
    read: () => JSON.parse(readFileSync("windows/src-tauri/tauri.conf.json", "utf8")).version,
  },
  {
    name: "windows/src-tauri/Cargo.toml",
    read: () => readCargoVersion("windows/src-tauri/Cargo.toml"),
  },
  {
    name: "macos/package.json",
    read: () => JSON.parse(readFileSync("macos/package.json", "utf8")).version,
  },
  {
    name: "macos/package-lock.json",
    read: () => JSON.parse(readFileSync("macos/package-lock.json", "utf8")).version,
  },
  {
    name: "macos/package-lock.json packages[\"\"]",
    read: () => JSON.parse(readFileSync("macos/package-lock.json", "utf8")).packages[""].version,
  },
  {
    name: "macos/src-tauri/tauri.conf.json",
    read: () => JSON.parse(readFileSync("macos/src-tauri/tauri.conf.json", "utf8")).version,
  },
  {
    name: "macos/src-tauri/Cargo.toml",
    read: () => readCargoVersion("macos/src-tauri/Cargo.toml"),
  },
];

function readCargoVersion(path) {
  const content = readFileSync(path, "utf8");
  const match = content.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error(`Could not find package version in ${path}`);
  }
  return match[1];
}

function readTopChangelogVersion() {
  const content = readFileSync("CHANGELOG.md", "utf8");
  const match = content.match(/^##\s+([0-9]+\.[0-9]+\.[0-9]+)/m);
  if (!match) {
    throw new Error("Could not find the latest CHANGELOG.md version heading");
  }
  return match[1];
}

const versions = files.map((file) => ({ name: file.name, version: file.read() }));
const expected = versions[0].version;
const mismatches = versions.filter((entry) => entry.version !== expected);
const changelogVersion = readTopChangelogVersion();

for (const entry of versions) {
  console.log(`${entry.name}: ${entry.version}`);
}
console.log(`CHANGELOG.md latest: ${changelogVersion}`);

if (mismatches.length > 0 || changelogVersion !== expected) {
  console.error("\nVersion consistency check failed.");

  for (const entry of mismatches) {
    console.error(`- ${entry.name} is ${entry.version}, expected ${expected}`);
  }

  if (changelogVersion !== expected) {
    console.error(`- CHANGELOG.md latest is ${changelogVersion}, expected ${expected}`);
  }

  process.exit(1);
}

console.log("\nVersion consistency check passed.");
