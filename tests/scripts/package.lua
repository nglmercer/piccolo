-- Exercises the package library: preload loaders, file loading via package.path,
-- module caching, package.searchpath, and the "module not found" error path.

-- 1. package.preload: register a loader; require calls it with the module name.
package.preload["greet"] = function(modname)
  return { loaded = modname, kind = "preload" }
end
local g = require("greet")
assert(g.loaded == "greet")
assert(g.kind == "preload")

-- 2. Caching: a second require returns the same table, stored in package.loaded.
g.marker = true
local g2 = require("greet")
assert(g2.marker == true)
assert(package.loaded["greet"] == g)

-- 3. A loader that returns nil is cached as `true`.
package.preload["nothing"] = function() return nil end
assert(require("nothing") == true)
assert(package.loaded["nothing"] == true)

-- 4. File loading via package.path: write a real module and require it from disk.
local base = os.tmpname() -- e.g. /tmp/piccolo_tmp_<hex>, no dots in the name
local w = io.open(base, "w")
assert(w)
assert(w:write("return { answer = 6 * 7, fromfile = true }"))
assert(w:close())

local oldpath = package.path
package.path = "?" -- module name is used verbatim as the file path
local m = require(base)
assert(m.answer == 42)
assert(m.fromfile == true)

-- 5. File modules are cached in package.loaded as well.
m.tag = "cached"
assert(require(base).tag == "cached")
assert(package.loaded[base] == m)
package.path = oldpath

-- 6. package.searchpath maps dotted names onto sub-paths and reports tried files.
local found, err = package.searchpath("foo.bar", "/no/such/dir/?.lua")
assert(found == nil and type(err) == "string")
assert(string.find(err, "/no/such/dir/foo/bar.lua", 1, true) ~= nil)

-- searchpath locates an existing file (base is still on disk, no dots in its name).
assert(package.searchpath(base, "?") == base)

-- 7. Requiring a module no searcher can find raises a "not found" error.
local ok, rerr = pcall(require, "definitely_not_a_module_xyz")
assert(ok == false)
assert(type(rerr) == "string")
assert(string.find(rerr, "not found", 1, true) ~= nil)
assert(string.find(rerr, "package.preload", 1, true) ~= nil)

os.remove(base)

print("package ok")
