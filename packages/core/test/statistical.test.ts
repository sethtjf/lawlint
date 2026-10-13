// Ported from crates/lawlint-core/src/engines/statistical.rs #[cfg(test)].

import { expect, test } from "bun:test";
import { spelled } from "../src/engines/statistical";

test("spelled_numbers", () => {
  expect(spelled(2)).toBe("Two");
  expect(spelled(3)).toBe("Three");
  expect(spelled(10)).toBe("Ten");
  expect(spelled(11)).toBe("11");
  expect(spelled(1)).toBe("1");
});
