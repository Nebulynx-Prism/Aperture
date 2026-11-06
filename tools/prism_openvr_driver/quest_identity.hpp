#pragma once
#include <stdint.h>
struct PrismQuestIdentity { const char* headset; const char* controller; const char* render_model; };
inline PrismQuestIdentity prismQuestIdentity(uint32_t type) {
    switch(type) {
    case 1: return {"Meta Quest", "Meta Touch", "oculus_quest_controller"};
    case 2: return {"Meta Quest 2", "Meta Touch", "oculus_quest2_controller"};
    case 3: return {"Meta Quest Pro", "Meta Touch Pro", "oculus_quest_pro_controller"};
    case 4: return {"Meta Quest 3", "Meta Touch Plus", "oculus_quest_plus_controller"};
    case 5: return {"Meta Quest 3S", "Meta Touch Plus", "oculus_quest_plus_controller"};
    default: return {"Meta Quest (connecting)", "Meta Touch", "oculus_quest2_controller"};
    }
}
