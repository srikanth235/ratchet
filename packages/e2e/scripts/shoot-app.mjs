// Screenshot every app screen: node scripts/shoot-app.mjs [outDir]   (run under xvfb-run, desktop built)
import { join } from 'node:path';
import { ARTIFACTS, SCREENS, goTo, launchApp, openNewOptimization, screenshotScreen, waitForScreen } from '../harness/launch.mjs';

const dir = process.argv[2] ? join(process.cwd(), process.argv[2]) : join(ARTIFACTS, 'app');
const { app, win, close } = await launchApp({ engine: 'dev' });
try {
  await win.waitForTimeout(1500);
  for (const s of SCREENS) {
    await goTo(win, s.nav);
    await waitForScreen(win, s.route);
    await win.waitForTimeout(1200);
    console.log(await screenshotScreen(app, win, s.name, dir));
  }
  await openNewOptimization(win);
  await win.waitForTimeout(800);
  console.log(await screenshotScreen(app, win, 'new-optimization', dir));
} finally {
  await close();
}
