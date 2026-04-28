function greet(name) {
  const cleanName = String(name || '').trim() || 'developer';
  return `Hello ${cleanName} from Node localci`;
}

module.exports = { greet };
