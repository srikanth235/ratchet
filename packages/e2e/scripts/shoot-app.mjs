// Screenshot every app screen: node scripts/shoot-app.mjs [outDir]   (run under xvfb-run, desktop built)
// The seeded dev engine; the three New optimization steps (a demo picked), and the Benchmarks Templates gallery are captured too.
import { join } from 'node:path';
import { ARTIFACTS, SCREENS, SCREENS_FROM_OPTIMIZATION, goTo, launchApp, openNewOptimization, openOptimization, openSandboxDetails, openTab, screenshotScreen, waitForScreen } from '../harness/launch.mjs';

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
  // The per-optimization screens: open the live run from the list, then its tabs.
  await openOptimization(win, 'search-api');
  for (const s of SCREENS_FROM_OPTIMIZATION) {
    await openTab(win, s.tab);
    await waitForScreen(win, s.route);
    await win.waitForTimeout(1200);
    console.log(await screenshotScreen(app, win, s.name, dir));
  }
  await openSandboxDetails(win);
  await win.waitForTimeout(800);
  console.log(await screenshotScreen(app, win, 'sandbox', dir));
  await openNewOptimization(win);
  await win.waitForTimeout(800);
  console.log(await screenshotScreen(app, win, 'new-optimization', dir));
  await win.getByRole('group', { name: 'Demos' }).getByRole('button').first().click();
  await win.getByRole('button', { name: 'Continue to review' }).click();
  await win.getByRole('heading', { name: 'Review', level: 2 }).waitFor();
  await win.waitForTimeout(800);
  console.log(await screenshotScreen(app, win, 'new-optimization-review', dir));
  await win.getByRole('button', { name: 'Continue to budget' }).click();
  await win.getByRole('heading', { name: 'Budget & time', level: 2 }).waitFor();
  console.log(await screenshotScreen(app, win, 'new-optimization-budget', dir));
  await goTo(win, 'Benchmarks');
  await win.getByRole('radio', { name: 'Templates' }).click();
  await win.getByRole('region', { name: 'Evaluator templates' }).waitFor();
  await win.waitForTimeout(500);
  console.log(await screenshotScreen(app, win, 'benchmarks-templates', dir));
} finally {
  await close();
}
