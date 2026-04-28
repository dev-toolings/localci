const assert = require('node:assert/strict');
const { greet } = require('../src/greeting');

assert.equal(greet('Ada'), 'Hello Ada from Node localci');
assert.equal(greet(''), 'Hello developer from Node localci');

console.log('Node tests passed');
