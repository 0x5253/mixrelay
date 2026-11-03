// Exercises the real XPC dictionary decoder with no root mutation or UI.
#import "../src/removal/native.m"
#include <assert.h>
int main(void) {
    @autoreleasepool {
        assert(!MRRemovalMessageAuthorized(xpc_string_create("not a dictionary")));
        xpc_object_t request = xpc_dictionary_create(NULL,NULL,0);
        assert(!MRRemovalMessageAuthorized(request));
        unsigned char unauthorized[sizeof(AuthorizationExternalForm)] = {0};
        xpc_dictionary_set_uint64(request,"version",1);
        xpc_dictionary_set_bool(request,"deleteSettings",false);
        xpc_dictionary_set_data(request,"authorization",unauthorized,sizeof(unauthorized));
        assert(!MRRemovalMessageAuthorized(request));
        AuthorizationRef unapproved = NULL;
        assert(AuthorizationCreate(NULL,NULL,kAuthorizationFlagDefaults,&unapproved) == 0);
        AuthorizationExternalForm external;
        assert(AuthorizationMakeExternalForm(unapproved,&external) == 0);
        xpc_dictionary_set_data(request,"authorization",&external,sizeof(external));
        assert(!MRRemovalMessageAuthorized(request));
        AuthorizationFree(unapproved,kAuthorizationFlagDestroyRights);
        xpc_dictionary_set_string(request,"path","/arbitrary");
        assert(!MRRemovalMessageAuthorized(request));
        xpc_dictionary_set_value(request,"path",NULL);
        xpc_dictionary_set_string(request,"deleteSettings","true");
        assert(!MRRemovalMessageAuthorized(request));
        xpc_dictionary_set_bool(request,"deleteSettings",true);
        xpc_dictionary_set_uint64(request,"version",2);
        assert(!MRRemovalMessageAuthorized(request));
        xpc_dictionary_set_uint64(request,"version",1);
        xpc_dictionary_set_data(request,"authorization",unauthorized,1);
        assert(!MRRemovalMessageAuthorized(request));
        puts("XPC protocol rejects malformed, over-scoped and unauthenticated requests");
    }
}
