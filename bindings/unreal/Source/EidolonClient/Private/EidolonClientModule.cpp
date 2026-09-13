// EidolonClientModule.cpp: Unreal Engine 5 Module Implementation

#include "IEidolonClientModule.h"

class FEidolonClientModule : public IEidolonClientModule
{
public:
    virtual void StartupModule() override
    {
    }

    virtual void ShutdownModule() override
    {
    }
};

IMPLEMENT_MODULE(FEidolonClientModule, EidolonClient)
