const fs = require("fs");
const { helper } = require("./util");
import def from "./mod.js";

class Animal {
  speak() { return helper(); }
}
class Dog extends Animal {
  speak() { return super.speak(); }
}
function run() {
  new Dog().speak();
  def();
}
module.exports = { run };
