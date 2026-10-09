import { describe, expect, it } from "vitest";
import { NEW_ENTRIES, newEntryError, newEntryKinds } from "../src/core/fileUploadsModel.js";

describe("the New menu's model", () => {
  it("offers a kind per announced verb, file first", () => {
    expect(newEntryKinds({ createDirectory: true, createFile: true })).toEqual(["file", "folder"]);
    expect(newEntryKinds({ createDirectory: true })).toEqual(["folder"]);
    expect(newEntryKinds({ createFile: true })).toEqual(["file"]);
    expect(newEntryKinds({ uploads: true })).toEqual([]);
    expect(newEntryKinds(null)).toEqual([]);
  });
  it("names each draft and the verb it sends", () => {
    expect(NEW_ENTRIES.file).toMatchObject({ label: "New file", field: "New file name", placeholder: "File name", method: "fs.createFile" });
    expect(NEW_ENTRIES.folder).toMatchObject({ label: "New folder", field: "New folder name", placeholder: "Folder name", method: "fs.createDirectory" });
  });
  it("refuses a taken file name in two words and keeps a folder's own sentence", () => {
    const taken = Object.assign(new Error("notes.md already exists"), { code: "already_exists" });
    expect(newEntryError("file", taken)).toBe("Already exists");
    expect(newEntryError("folder", taken)).toBe("notes.md already exists");
    expect(newEntryError("file", { error: { code: "already_exists" } })).toBe("Already exists");
    expect(newEntryError("file", new Error("cannot create file: Permission denied"))).toBe("cannot create file: Permission denied");
    expect(newEntryError("file", {})).toBe("Could not create file.");
    expect(newEntryError("folder", {})).toBe("Could not create folder.");
  });
});
