import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function electronBuilderArguments({
  platform,
  signingConfigured,
  commandArguments,
  localUnsigned = false,
}) {
  if (localUnsigned) {
    const args = [...commandArguments, "--publish", "never"];
    if (platform === "darwin") {
      args.push("--config.mac.identity=-", "--config.mac.notarize=false", "--config.mac.hardenedRuntime=false");
    }
    return args;
  }
  if (platform !== "darwin" || signingConfigured) return commandArguments;
  return [...commandArguments, "--config.mac.identity=-"];
}

export function unsignedEnvironment(environment) {
  const clean = Object.fromEntries(Object.entries(environment).filter(
    ([key]) => !/^(CSC_|WIN_CSC_|APPLE_)/i.test(key),
  ));
  return { ...clean, CSC_IDENTITY_AUTO_DISCOVERY: "false" };
}

function macSigningConfigured(environment) {
  if (environment.CSC_LINK || environment.CSC_NAME) return true;

  const identities = spawnSync(
    "security",
    ["find-identity", "-v", "-p", "codesigning"],
    { encoding: "utf8" },
  );
  return identities.status === 0 && !identities.stdout.includes("0 valid identities found");
}

function packageApp() {
  const localUnsigned = process.argv.includes("--local-unsigned");
  const environment = localUnsigned ? unsignedEnvironment(process.env) : process.env;
  const scriptDirectory = dirname(fileURLToPath(import.meta.url));
  const projectDirectory = resolve(scriptDirectory, "..");
  const electronBuilder = join(
    projectDirectory,
    "node_modules",
    "electron-builder",
    "cli.js",
  );
  const commandArguments = electronBuilderArguments({
    platform: process.platform,
    signingConfigured:
      !localUnsigned && process.platform === "darwin" && macSigningConfigured(environment),
    commandArguments: process.argv.slice(2).filter((argument) => argument !== "--local-unsigned"),
    localUnsigned,
  });
  const result = spawnSync(
    process.execPath,
    [electronBuilder, ...commandArguments],
    { cwd: projectDirectory, env: environment, stdio: "inherit" },
  );

  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
}

if (resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) packageApp();
