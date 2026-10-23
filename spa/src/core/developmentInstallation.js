import { BUILD_CHANNEL } from "../config.js";

export const isDevelopmentChannel = () => BUILD_CHANNEL === "dev";

export const DEV_INSTALLATION_NOTICE = "Release installation is unavailable on the dev stack. Build the dev bridge from reviewed source into a distinct dev binary. Pairing and service setup require separate authorization; follow the dev setup documentation.";

export const developmentInstallationHtml = () =>
  `<p class="dim">${DEV_INSTALLATION_NOTICE}</p><a href="/docs#setup">Dev setup documentation</a>`;
