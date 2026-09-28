import { describe, expect, test } from "vitest";
import { fieldCls, inputCls } from "./ui";

/**
 * iOS Safari zooms the page in on focus when an editable element computes to
 * a font-size under 16px, so form inputs must default to text-base (16px)
 * and can only drop to text-sm at a `sm:` breakpoint or wider.
 */
function assertNoMobileZoomFont(cls: string) {
  expect(cls).toMatch(/(?:^|\s)text-base(?:\s|$)/);
  expect(cls).not.toMatch(/(?:^|\s)text-sm(?:\s|$)/);
}

describe("shared form field classes", () => {
  test("inputCls does not trigger iOS Safari focus zoom on phones", () => {
    assertNoMobileZoomFont(inputCls);
  });

  test("fieldCls does not trigger iOS Safari focus zoom on phones", () => {
    assertNoMobileZoomFont(fieldCls);
  });
});
