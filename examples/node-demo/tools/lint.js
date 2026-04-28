const { execFileSync } = require('node:child_process');

for (const file of ['src/greeting.js', 'src/cli.js', 'test/greeting.test.js', 'tools/lint.js']) {
  execFileSync(process.execPath, ['--check', file], { stdio: 'inherit' });
}

console.log('Node lint passed');
