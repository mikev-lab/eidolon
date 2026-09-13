// EidolonSubsystem.cpp: Implementation of Unreal Engine Subsystem

#include "EidolonSubsystem.h"
#include "EidolonSettings.h"
#include "eidolon.h"

UEidolonSubsystem::UEidolonSubsystem()
    : ClientHandle(nullptr)
{
}

void UEidolonSubsystem::Initialize(FSubsystemCollectionBase& Collection)
{
    Super::Initialize(Collection);

    ClientHandle = eidolon_client_create();

    // Register ticker to poll events on the game thread every frame
    TickerHandle = FTSTicker::GetCoreTicker().AddTicker(
        FTickerDelegate::CreateUObject(this, &UEidolonSubsystem::OnTick)
    );
}

void UEidolonSubsystem::Deinitialize()
{
    if (TickerHandle.IsValid())
    {
        FTSTicker::GetCoreTicker().RemoveTicker(TickerHandle);
        TickerHandle.Reset();
    }

    if (ClientHandle != nullptr)
    {
        eidolon_client_destroy(ClientHandle);
        ClientHandle = nullptr;
    }

    Super::Deinitialize();
}

bool UEidolonSubsystem::Connect(FString Host, int32 Port, int64 AccountId)
{
    if (ClientHandle == nullptr)
    {
        return false;
    }

    const UEidolonSettings* Settings = GetDefault<UEidolonSettings>();

    FString TargetHost = Host.IsEmpty() ? (Settings ? Settings->ServerHost : TEXT("127.0.0.1")) : Host;
    int32 TargetPort = Port <= 0 ? (Settings ? Settings->ServerPort : 7777) : Port;
    int64 TargetAccount = AccountId <= 0 ? (Settings ? Settings->DefaultAccountId : 1001) : AccountId;

    FTCHARToUTF8 HostUtf8(*TargetHost);

    int32 Result = eidolon_client_connect(
        ClientHandle,
        HostUtf8.Get(),
        static_cast<uint16_t>(TargetPort),
        static_cast<uint64_t>(TargetAccount),
        0,
        0
    );

    if (Result == EIDOLON_OK && Settings != nullptr)
    {
        eidolon_client_set_density_profile(ClientHandle, static_cast<uint32_t>(Settings->DensityProfile));
    }

    return Result == EIDOLON_OK;
}

void UEidolonSubsystem::Disconnect()
{
    if (ClientHandle != nullptr)
    {
        eidolon_client_disconnect(ClientHandle);
    }
}

bool UEidolonSubsystem::IsConnected() const
{
    if (ClientHandle != nullptr)
    {
        return eidolon_client_is_connected(ClientHandle) == 1;
    }
    return false;
}

bool UEidolonSubsystem::SendMovementIntent(FVector Velocity, float YawDeg, uint8 Flags)
{
    if (ClientHandle == nullptr || !IsConnected())
    {
        return false;
    }

    const UEidolonSettings* Settings = GetDefault<UEidolonSettings>();
    EEidolonCoordinateConvention Convention = Settings ? Settings->CoordinateConvention : EEidolonCoordinateConvention::UnrealStandard;

    float vx = 0.0f, vy = 0.0f, vz = 0.0f;
    FEidolonCoordinateConversion::ToEidolonPosition(Velocity, vx, vy, vz, Convention);

    return eidolon_client_send_intent(ClientHandle, vx, vz, YawDeg, Flags) == EIDOLON_OK;
}

bool UEidolonSubsystem::CastAbility(int64 TargetEntityId, int32 AbilityId)
{
    if (ClientHandle == nullptr || !IsConnected())
    {
        return false;
    }

    return eidolon_client_cast_ability(ClientHandle, static_cast<uint32_t>(TargetEntityId), static_cast<uint32_t>(AbilityId)) == EIDOLON_OK;
}

bool UEidolonSubsystem::EquipItem(uint8 InventorySlot, uint8 EquipSlot)
{
    if (ClientHandle == nullptr || !IsConnected())
    {
        return false;
    }

    return eidolon_client_equip_item(ClientHandle, InventorySlot, EquipSlot) == EIDOLON_OK;
}

bool UEidolonSubsystem::SendChatMessage(uint8 Channel, int64 TargetId, const FString& Text)
{
    if (ClientHandle == nullptr || !IsConnected())
    {
        return false;
    }

    FTCHARToUTF8 TextUtf8(*Text);
    return eidolon_client_send_chat(ClientHandle, Channel, static_cast<uint32_t>(TargetId), TextUtf8.Get()) == EIDOLON_OK;
}

bool UEidolonSubsystem::SendVehicleIntent(const FEidolonVehicleIntent& Intent)
{
    if (ClientHandle == nullptr || !IsConnected())
    {
        return false;
    }

    EidolonVehicleIntent NativeIntent{};
    NativeIntent.vehicle_type = Intent.VehicleType;
    NativeIntent.throttle = Intent.Throttle;
    NativeIntent.steering = Intent.Steering;
    NativeIntent.pitch = Intent.Pitch;
    NativeIntent.roll = Intent.Roll;

    return eidolon_client_send_vehicle_intent(ClientHandle, &NativeIntent) == EIDOLON_OK;
}

bool UEidolonSubsystem::QueryTerrainHeight(int32 SectorX, int32 SectorZ, float LocalX, float LocalZ, float& OutElevation)
{
    OutElevation = 0.0f;
    if (ClientHandle == nullptr)
    {
        return false;
    }

    return eidolon_client_query_terrain_height(ClientHandle, SectorX, SectorZ, LocalX, LocalZ, &OutElevation) == EIDOLON_OK;
}

bool UEidolonSubsystem::OnTick(float DeltaTime)
{
    if (ClientHandle == nullptr)
    {
        return true;
    }

    const UEidolonSettings* Settings = GetDefault<UEidolonSettings>();
    EEidolonCoordinateConvention Convention = Settings ? Settings->CoordinateConvention : EEidolonCoordinateConvention::UnrealStandard;

    auto EventCallback = [](const EidolonEvent* Evt, void* UserData)
    {
        if (Evt == nullptr || UserData == nullptr)
        {
            return;
        }

        auto* Subsystem = static_cast<UEidolonSubsystem*>(UserData);
        const UEidolonSettings* CurrentSettings = GetDefault<UEidolonSettings>();
        EEidolonCoordinateConvention CurrentConvention = CurrentSettings ? CurrentSettings->CoordinateConvention : EEidolonCoordinateConvention::UnrealStandard;

        switch (Evt->event_type)
        {
            case EIDOLON_EVENT_CONNECTED:
                Subsystem->OnConnected.Broadcast(static_cast<int64>(Evt->param1));
                break;

            case EIDOLON_EVENT_DISCONNECTED:
                Subsystem->OnDisconnected.Broadcast(static_cast<int32>(Evt->param1));
                break;

            case EIDOLON_EVENT_ENTITY_SPAWNED:
            {
                FVector Pos = FEidolonCoordinateConversion::ToUnrealPosition(Evt->x, Evt->y, Evt->z, CurrentConvention);
                FRotator Rot = FEidolonCoordinateConversion::ToUnrealRotation(Evt->yaw_degrees);
                Subsystem->OnEntitySpawned.Broadcast(Evt->entity_id, static_cast<uint8>(Evt->param1), Pos, Rot);
                break;
            }

            case EIDOLON_EVENT_ENTITY_UPDATED:
            {
                FVector Pos = FEidolonCoordinateConversion::ToUnrealPosition(Evt->x, Evt->y, Evt->z, CurrentConvention);
                FRotator Rot = FEidolonCoordinateConversion::ToUnrealRotation(Evt->yaw_degrees);
                Subsystem->OnEntityUpdated.Broadcast(Evt->entity_id, Pos, Rot, FVector::ZeroVector);
                break;
            }

            case EIDOLON_EVENT_ENTITY_DESPAWNED:
                Subsystem->OnEntityDespawned.Broadcast(Evt->entity_id);
                break;

            case EIDOLON_EVENT_CHAT_MESSAGE:
                Subsystem->OnChatMessageReceived.Broadcast(static_cast<uint8>(Evt->param1), static_cast<int64>(Evt->param2), TEXT("Message received"));
                break;

            case EIDOLON_EVENT_TIME_DILATION_CHANGED:
                Subsystem->OnTimeDilationChanged.Broadcast(Evt->x);
                break;

            default:
                break;
        }
    };

    eidolon_client_poll_events(ClientHandle, EventCallback, this);

    return true; // Keep ticker active
}
