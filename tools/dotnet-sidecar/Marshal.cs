// Tagged values on the wire (ADR-014 section 4) and their conversion to and from .NET objects.
//
//   {"t":"n"}                              null
//   {"t":"b","v":true}                     bool
//   {"t":"u8","v":200}                     byte
//   {"t":"c","v":65}                       char (UTF-16 code unit)
//   {"t":"i4","v":1}  {"t":"i8","v":1}     int, long
//   {"t":"d","v":"1.50"}                   decimal, as text
//   {"t":"s","v":"text"}                   string
//   {"t":"dt","v":"638396640000000000"}    DateTime ticks, kind unspecified
//   {"t":"ts","v":"15000000"}              TimeSpan ticks
//   {"t":"g","v":"01234567-89ab-..."}      Guid, in textual order
//   {"t":"e","type":"System.X","v":"Name","o":1}   enum value by name and ordinal
//   {"t":"h","v":12,"type":"System.IO.MemoryStream"}   object held here
//   {"t":"ins","v":"<base64>"}             AL InStream: the unread bytes
//   {"t":"outs"}                           AL OutStream: becomes a bridge stream
using System.Globalization;
using System.Text.Json.Nodes;

namespace AlDotNetSidecar;

public sealed class Marshaller
{
    readonly Dictionary<long, object> handles = new();
    long nextHandle = 1;
    readonly Func<string, Type> resolveType;

    // Streams passed in the current request, and bridges that live until the process ends.
    public sealed class InArg { public int Index; public MemoryStream Stream = null!; }
    public readonly List<InArg> Ins = new();
    public readonly List<(int Index, long Bridge)> NewBridges = new();
    readonly Dictionary<long, (MemoryStream Stream, int Drained)> bridges = new();
    long nextBridge = 1;

    public Marshaller(Func<string, Type> resolveType) { this.resolveType = resolveType; }

    public int HandleCount => handles.Count;

    public object Handle(long h) =>
        handles.TryGetValue(h, out var o) ? o : throw new Refusal($"unknown object handle {h}");

    public void Release(JsonArray hs)
    {
        foreach (var h in hs) handles.Remove(h!.GetValue<long>());
    }

    public void BeginRequest()
    {
        Ins.Clear();
        NewBridges.Clear();
    }

    /// Bytes written into any bridge since the last call, as [[id, base64], ...].
    public JsonArray DrainBridges()
    {
        var arr = new JsonArray();
        foreach (var id in bridges.Keys.ToList())
        {
            var b = bridges[id];
            var all = b.Stream.ToArray();
            if (all.Length > b.Drained)
            {
                arr.Add(new JsonArray(id, Convert.ToBase64String(all, b.Drained, all.Length - b.Drained)));
                bridges[id] = (b.Stream, all.Length);
            }
        }
        return arr;
    }

    // ---- wire to .NET ----

    public object? Decode(JsonNode? n, int index)
    {
        if (n == null) return null;
        var o = n.AsObject();
        switch (o["t"]!.GetValue<string>())
        {
            case "n": return null;
            case "b": return o["v"]!.GetValue<bool>();
            case "u8": return (byte)o["v"]!.GetValue<int>();
            case "c": return (char)o["v"]!.GetValue<int>();
            case "i4": return o["v"]!.GetValue<int>();
            case "i8": return o["v"]!.GetValue<long>();
            case "d": return decimal.Parse(o["v"]!.GetValue<string>(), NumberStyles.Float, CultureInfo.InvariantCulture);
            case "s": return o["v"]!.GetValue<string>();
            case "dt": return new DateTime(long.Parse(o["v"]!.GetValue<string>(), CultureInfo.InvariantCulture), DateTimeKind.Unspecified);
            case "ts": return new TimeSpan(long.Parse(o["v"]!.GetValue<string>(), CultureInfo.InvariantCulture));
            case "g": return Guid.Parse(o["v"]!.GetValue<string>());
            case "e": return Enum.Parse(resolveType(o["type"]!.GetValue<string>()), o["v"]!.GetValue<string>());
            case "h": return Handle(o["v"]!.GetValue<long>());
            case "ins":
            {
                var ms = new MemoryStream(Convert.FromBase64String(o["v"]!.GetValue<string>()), writable: false);
                Ins.Add(new InArg { Index = index, Stream = ms });
                return ms;
            }
            case "outs":
            {
                var ms = new MemoryStream();
                long id = nextBridge++;
                bridges[id] = (ms, 0);
                NewBridges.Add((index, id));
                return ms;
            }
            default: throw new Refusal("unknown value tag " + o["t"]);
        }
    }

    // ---- .NET to wire ----

    public JsonNode Encode(object? v)
    {
        switch (v)
        {
            case null: return Tag("n");
            case string s: return Tag("s", s);
            case bool b: return Tag("b", b);
            case int i: return Tag("i4", i);
            case long l: return Tag("i8", l);
            case byte u: return Tag("u8", (int)u);
            case char c: return Tag("c", (int)c);
            case short sh: return Tag("i4", (int)sh);
            case ushort us: return Tag("i4", (int)us);
            case sbyte sb: return Tag("i4", (int)sb);
            case uint ui: return Tag("i8", (long)ui);
            case decimal m: return Tag("d", m.ToString(CultureInfo.InvariantCulture));
            case double d:
                return Tag("d", ToDecimal(d).ToString(CultureInfo.InvariantCulture));
            case float f:
                return Tag("d", ToDecimal(f).ToString(CultureInfo.InvariantCulture));
            case DateTime dt: return Tag("dt", dt.Ticks.ToString(CultureInfo.InvariantCulture));
            case TimeSpan ts: return Tag("ts", ts.Ticks.ToString(CultureInfo.InvariantCulture));
            case Guid g: return Tag("g", g.ToString("D"));
            case Enum e:
            {
                var o = Tag("e", e.ToString());
                o["type"] = e.GetType().FullName;
                o["o"] = Convert.ToInt64(e, CultureInfo.InvariantCulture);
                return o;
            }
            default:
            {
                long h = nextHandle++;
                handles[h] = v;
                var o = Tag("h", h);
                o["type"] = v.GetType().FullName;
                return o;
            }
        }
    }

    static decimal ToDecimal(double d)
    {
        try { return (decimal)d; }
        catch (OverflowException) { throw new Refusal($"the .NET number {d.ToString(CultureInfo.InvariantCulture)} does not fit an AL Decimal"); }
    }

    static JsonObject Tag(string t, JsonNode? v = null)
    {
        var o = new JsonObject { ["t"] = t };
        if (v != null) o["v"] = v;
        return o;
    }
}

/// A request this process cannot serve: type or member not found, no matching overload. It is
/// reported as a refusal, not as an exception of the code under test (ADR-014 section 7).
public sealed class Refusal : Exception
{
    public Refusal(string message) : base(message) { }
}
