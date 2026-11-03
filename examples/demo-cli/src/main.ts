#!/usr/bin/env node
import { main } from './cli.js';

process.exitCode = await main(
  process.argv.slice(2),
  { out: (s) => void process.stdout.write(s), err: (s) => void process.stderr.write(s) },
  process.cwd(),
  process.env,
);
