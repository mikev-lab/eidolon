// EidolonGodotConfig.cs: Godot 4 Resource Configuration Asset

using Godot;

namespace Eidolon.GodotEngine
{
    [GlobalClass]
    public partial class EidolonGodotConfig : Resource
    {
        [Export] public string ServerHost { get; set; } = "127.0.0.1";
        [Export] public int ServerPort { get; set; } = 7777;
        [Export] public ulong DefaultAccountId { get; set; } = 1001;
        [Export] public ulong TicketHigh { get; set; } = 0;
        [Export] public ulong TicketLow { get; set; } = 0;
        [Export(PropertyHint.Range, "0,2")] public uint DensityProfile { get; set; } = 1; // Standard MMO
        [Export(PropertyHint.Range, "0.01,0.20")] public float InterpolationDelay { get; set; } = 0.05f;
        [Export(PropertyHint.Range, "1.0,50.0")] public float SnapDistanceThreshold { get; set; } = 10.0f;
    }
}
