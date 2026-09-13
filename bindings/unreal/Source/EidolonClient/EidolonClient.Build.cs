// EidolonClient.Build.cs: Unreal Build Tool Module Rules for Eidolon Client Plugin
// Compatible with Unreal Engine 5.0 through 5.4+.

using System.IO;
using UnrealBuildTool;

public class EidolonClient : ModuleRules
{
    public EidolonClient(ReadOnlyTargetRules Target) : base(Target)
    {
        PCHUsage = ModuleRules.PCHUsageMode.UseExplicitOrSharedPCHs;

        PublicDependencyModuleNames.AddRange(
            new string[]
            {
                "Core",
                "CoreUObject",
                "Engine",
                "InputCore",
                "DeveloperSettings"
            }
        );

        PrivateDependencyModuleNames.AddRange(
            new string[]
            {
                "Slate",
                "SlateCore"
            }
        );

        // Native Eidolon C ABI header path
        string PluginRoot = Path.GetFullPath(Path.Combine(ModuleDirectory, "../../.."));
        string IncludePath = Path.Combine(PluginRoot, "include");
        PublicIncludePaths.Add(IncludePath);

        // Platform-specific native library linkage
        string LibDir = Path.Combine(PluginRoot, "target", "release");

        if (Target.Platform == UnrealTargetPlatform.Win64)
        {
            PublicAdditionalLibraries.Add(Path.Combine(LibDir, "eidolon.lib"));
            PublicDelayLoadDLLs.Add("eidolon.dll");
        }
        else if (Target.Platform == UnrealTargetPlatform.Mac)
        {
            PublicAdditionalLibraries.Add(Path.Combine(LibDir, "libeidolon.a"));
        }
        else if (Target.Platform == UnrealTargetPlatform.Linux)
        {
            PublicAdditionalLibraries.Add(Path.Combine(LibDir, "libeidolon.a"));
        }
    }
}
