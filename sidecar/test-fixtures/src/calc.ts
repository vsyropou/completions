import { helperAdd, Point } from "./utils";

export class Calculator {
  add(a: number, b: number) {
    return helperAdd(a, b);
  }
}

export function makePoint(x: number, y: number): Point {
  return { x, y };
}
