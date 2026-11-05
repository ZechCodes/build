// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: vi.fn(() => ({ bodies: { pages: true } })) }));
import { bridgeCapabilities } from "../src/core/changeEvents.js";
import { downloadFile } from "../src/core/fileDownload.js";

const encoded = (bytes) => btoa(String.fromCharCode(...bytes));
const answer = (bytes, extra = {}) => ({ content_b64: encoded(bytes), size: bytes.length, mime: "application/octet-stream", truncated: false, ...extra });
const page = (bytes, offset, total, version = "a") => answer(bytes, { range: { offset, end: offset + bytes.length, total, version } });

describe("file downloads", () => {
  let click;
  let createUrl;
  beforeEach(() => {
    vi.restoreAllMocks();
    bridgeCapabilities.mockReturnValue({ bodies: { pages: true } });
    click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    createUrl = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:download");
  });
  it("saves binary bytes under the file name and preserves the scoped RPC", async () => {
    const rpc = vi.fn().mockResolvedValue(answer([0, 255, 128, 13]));
    let link;
    click.mockImplementation(function () { link = { name: this.download, href: this.href }; });
    await downloadFile({ callRpc: rpc, scope: { project_id: "p", source_id: "s" }, path: "dir/data.bin", deviceId: "d" });
    expect(rpc).toHaveBeenCalledWith("fs.read", { project_id: "p", source_id: "s", path: "dir/data.bin" });
    expect(link).toEqual({ name: "data.bin", href: "blob:download" });
    expect(createUrl.mock.calls[0][0].size).toBe(4);
    const bytes = await new Promise((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve([...new Uint8Array(reader.result)]);
      reader.readAsArrayBuffer(createUrl.mock.calls[0][0]);
    });
    expect(bytes).toEqual([0, 255, 128, 13]);
    expect(document.querySelector("a[download]")).toBeNull();
  });
  it("rereads a truncated file in contiguous versioned pages", async () => {
    const rpc = vi.fn().mockResolvedValueOnce(answer([0], { truncated: true, size: 4 }))
      .mockResolvedValueOnce(page([0, 255], 0, 4)).mockResolvedValueOnce(page([128, 13], 2, 4));
    await downloadFile({ callRpc: rpc, scope: { workspace_id: "w" }, path: "data.bin", deviceId: "d" });
    expect(rpc.mock.calls[1][1].range).toEqual({ offset: 0, bytes: 1048576 });
    expect(rpc.mock.calls[2][1].range.offset).toBe(2);
    expect(createUrl.mock.calls[0][0].size).toBe(4);
    const bytes = await new Promise((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve([...new Uint8Array(reader.result)]);
      reader.readAsArrayBuffer(createUrl.mock.calls[0][0]);
    });
    expect(bytes).toEqual([0, 255, 128, 13]);
  });
  it("refuses a truncated legacy result instead of saving a partial file", async () => {
    bridgeCapabilities.mockReturnValue({});
    await expect(downloadFile({ callRpc: vi.fn().mockResolvedValue(answer([1], { truncated: true })), path: "a" })).rejects.toThrow(/Update the bridge/);
    expect(click).not.toHaveBeenCalled();
  });
  it.each([
    page([3, 4], 2, 4, "changed"),
    page([3, 4], 1, 4),
    page([], 2, 4),
  ])("refuses changed or incomplete pages", async (last) => {
    const rpc = vi.fn().mockResolvedValueOnce(answer([1], { truncated: true }))
      .mockResolvedValueOnce(page([1, 2], 0, 4)).mockResolvedValueOnce(last);
    await expect(downloadFile({ callRpc: rpc, path: "a", deviceId: "d" })).rejects.toThrow();
    expect(click).not.toHaveBeenCalled();
  });
});
