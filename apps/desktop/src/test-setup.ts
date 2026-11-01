import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// globals を使わないので、後始末は自分で登録する
afterEach(() => cleanup());
