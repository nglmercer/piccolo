-- Exercises the os library. Time values that depend on the host clock are only
-- checked for type/range; calendar conversions are checked against fixed UTC values.

-- os.clock returns a non-negative number of seconds.
local c = os.clock()
assert(type(c) == "number" and c >= 0)

-- os.time() with no args returns the current time as a number.
local now = os.time()
assert(type(now) == "number" and now > 0)

-- os.time(table) converts a broken-down UTC time to a timestamp.
-- 2000-01-01 00:00:00 UTC == 946684800
local t = os.time({ year = 2000, month = 1, day = 1, hour = 0, min = 0, sec = 0 })
assert(t == 946684800, "time() got " .. tostring(t))

-- os.date with '!' (UTC) formats a known timestamp deterministically.
assert(os.date("!%Y-%m-%d", 946684800) == "2000-01-01")
assert(os.date("!%H:%M:%S", 946684800) == "00:00:00")
assert(os.date("!%Y", 946684800) == "2000")

-- os.date / os.time round-trip.
local rt = os.time({ year = 2021, month = 6, day = 15, hour = 12, min = 30, sec = 45 })
assert(os.date("!%Y-%m-%d %H:%M:%S", rt) == "2021-06-15 12:30:45")

-- os.difftime
assert(os.difftime(100, 40) == 60)

-- os.getenv: an unset variable yields nil.
assert(os.getenv("PICCOLO_DEFINITELY_UNSET_VARIABLE_XYZ") == nil)

-- os.tmpname returns a non-empty string.
local tmp = os.tmpname()
assert(type(tmp) == "string" and #tmp > 0)

-- os.remove on a non-existent file fails gracefully (nil + message).
local ok, err = os.remove("/piccolo/definitely/does/not/exist.txt")
assert(ok == nil and type(err) == "string")

print("os ok")
