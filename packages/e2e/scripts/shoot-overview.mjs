// Screenshot the Overview over the seeded dev engine: node scripts/shoot-overview.mjs  (xvfb-run, desktop built)
// Writes artifacts/overview.png, to compare against design/v02/1-overview.png.
import { ARTIFACTS, goTo, launchApp, screenshotScreen, waitForScreen } from '../harness/launch.mjs';

const { app, win, close } = await launchApp({ engine: 'dev' });
try {
  await win.waitForTimeout(1500);
  await goTo(win, 'Overview');
  await waitForScreen(win, 'overview');
  await win.waitForTimeout(1500);
  console.log(await screenshotScreen(app, win, 'overview', ARTIFACTS));
} finally {
  await close();
}
