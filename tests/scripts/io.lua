-- Exercises the io/file library using a temporary file on disk.

local path = os.tmpname()
assert(type(path) == "string" and #path > 0)

-- io.type on non-file values.
assert(io.type(nil) == nil)
assert(io.type(42) == nil)
assert(io.type("not a file") == nil)

-- Open for writing, write several values, then close.
local w = io.open(path, "w")
assert(io.type(w) == "file")
-- write accepts strings and numbers, concatenating them.
assert(w:write("hello", " ", "world", "\n"))
assert(w:write(123, " ", 4.5, "\n"))
assert(w:write("last line"))
assert(w:close())

-- After closing, io.type reports a closed file.
assert(io.type(w) == "closed file")

-- Reopen for reading and read the whole file.
local r = io.open(path, "r")
assert(io.type(r) == "file")
local all = r:read("*a")
assert(all == "hello world\n123 4.5\nlast line", "read *a got: " .. tostring(all))

-- Reading at EOF returns nil.
assert(r:read("*a") == "")

-- Seek back to the start and read line by line.
assert(r:seek("set", 0))
assert(r:read("*l") == "hello world")
assert(r:read("*l") == "123 4.5")
assert(r:read("*l") == "last line")
assert(r:read("*l") == nil)

-- Seek back and read a fixed number of bytes.
assert(r:seek("set", 0))
assert(r:read(5) == "hello")

-- Seek back and read a number.
assert(r:seek("set", 0))
r:read("*l") -- skip "hello world"
local n = r:read("*n")
assert(n == 123, "read *n got: " .. tostring(n))

-- seek("cur") and seek("end") report positions.
assert(r:seek("set", 0) == 0)
assert(r:seek("cur", 3) == 3)
local size = r:seek("end")
assert(size == #all, "seek end got " .. tostring(size))

assert(r:close())

-- io.lines iterates over the lines of a file, closing it at EOF.
local collected = {}
for line in io.lines(path) do
  collected[#collected + 1] = line
end
assert(#collected == 3)
assert(collected[1] == "hello world")
assert(collected[2] == "123 4.5")
assert(collected[3] == "last line")

-- Append mode adds to the end of the file.
local a = io.open(path, "a")
assert(a:write("\nappended"))
assert(a:close())
local r2 = io.open(path, "r")
local all2 = r2:read("*a")
r2:close()
assert(all2 == "hello world\n123 4.5\nlast line\nappended")

-- io.tmpfile yields a usable, seekable file handle.
local tf = io.tmpfile()
if tf then
  assert(io.type(tf) == "file")
  assert(tf:write("abc"))
  assert(tf:seek("set", 0) == 0)
  assert(tf:read("*a") == "abc")
  assert(tf:close())
end

-- Opening a non-existent file for reading fails with nil + message.
local bad, err = io.open("/piccolo/definitely/does/not/exist.txt", "r")
assert(bad == nil and type(err) == "string")

-- Clean up the temp file.
os.remove(path)

print("io ok")
