#!/usr/bin/env node
import { processIo } from './io.js';
import { runCli } from './program.js';

process.exitCode = await runCli(process.argv.slice(2), processIo);
