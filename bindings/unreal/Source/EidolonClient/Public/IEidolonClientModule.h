// IEidolonClientModule.h: Unreal Engine 5 Module Interface for Eidolon Client

#pragma once

#include "CoreMinimal.h"
#include "Modules/ModuleManager.h"

class IEidolonClientModule : public IModuleInterface
{
public:
    static inline IEidolonClientModule& Get()
    {
        return FModuleManager::LoadModuleChecked<IEidolonClientModule>("EidolonClient");
    }

    static inline bool IsAvailable()
    {
        return FModuleManager::Get().IsModuleLoaded("EidolonClient");
    }
};
