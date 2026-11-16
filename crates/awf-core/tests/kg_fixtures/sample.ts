import { helper as h, other } from "./util";
import * as path from "node:path";
import Base from "@/base";

export interface Shape { area(): number }

// @awf:billing/export-invoice
export class Circle extends Base implements Shape {
  area(): number {
    return h(this.radius()) + other();
  }
  radius(): number { return 1; }
}

export const build = (n: number) => new Circle();

export function main() {
  build(1);
  path.join("a", "b");
}
