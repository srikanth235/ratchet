import { GetCallerIdentityCommand, STSClient } from '@aws-sdk/client-sts';

export interface CallerIdentity {
  accountId: string;
  arn: string;
}

export interface CheckAwsCredentialsOptions {
  region: string;
  /** Named profile in ~/.aws/config (static keys, SSO or assume-role). Default credential chain when unset. */
  profile?: string;
  /** The account the sandbox must live in; a mismatch is an error. */
  expectedAccountId?: string;
  client?: Pick<STSClient, 'send'>;
}

/**
 * "Who am I" for the app's own AWS credentials (STS GetCallerIdentity): proves the profile resolves, an SSO
 * session has not expired, and optionally that it is the dedicated sandbox account. Read-only.
 */
export async function checkAwsCredentials(opts: CheckAwsCredentialsOptions): Promise<CallerIdentity> {
  const client = opts.client ?? new STSClient({ region: opts.region, ...(opts.profile ? { profile: opts.profile } : {}) });
  const r = await client.send(new GetCallerIdentityCommand({}));
  const accountId = r.Account ?? '';
  if (!accountId) throw new Error('STS returned no account id');
  if (opts.expectedAccountId && accountId !== opts.expectedAccountId) {
    throw new Error(`credentials are for account ${accountId}, expected sandbox account ${opts.expectedAccountId}`);
  }
  return { accountId, arn: r.Arn ?? '' };
}
