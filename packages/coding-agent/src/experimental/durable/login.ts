import type { AuthCheck, AuthType } from "@at-inc/pi-ai";

interface LoginProviderDefinition {
	id: string;
	name: string;
	oauth?: { name: string; subscription?: boolean };
	apiKey?: { name: string; interactive: boolean };
}

interface ProviderAuthStatus {
	configured: boolean;
	label?: string;
	source?: string;
}

export interface LoginProviderOption {
	id: string;
	name: string;
	authType: AuthType;
	status?: AuthCheck;
	interactive: boolean;
	methodName: string;
	subscription?: boolean;
}

export function createLoginProviderOptions(
	providers: readonly LoginProviderDefinition[],
	getAuthStatus: (providerId: string) => ProviderAuthStatus,
	isUsingOAuth: (providerId: string) => boolean,
): LoginProviderOption[] {
	const options: LoginProviderOption[] = [];
	for (const provider of providers) {
		const authStatus = getAuthStatus(provider.id);
		const status = authStatus.configured
			? {
					type: isUsingOAuth(provider.id) ? ("oauth" as const) : ("api_key" as const),
					...((authStatus.label ?? authStatus.source) ? { source: authStatus.label ?? authStatus.source } : {}),
				}
			: undefined;
		if (provider.oauth) {
			options.push({
				id: provider.id,
				name: provider.name,
				authType: "oauth",
				...(status === undefined ? {} : { status }),
				interactive: true,
				methodName: provider.oauth.name,
				...(provider.oauth.subscription === undefined ? {} : { subscription: provider.oauth.subscription }),
			});
		}
		if (provider.apiKey) {
			options.push({
				id: provider.id,
				name: provider.name,
				authType: "api_key",
				...(status === undefined ? {} : { status }),
				interactive: provider.apiKey.interactive,
				methodName: provider.apiKey.name,
			});
		}
	}
	return options.sort((left, right) => left.name.localeCompare(right.name));
}
