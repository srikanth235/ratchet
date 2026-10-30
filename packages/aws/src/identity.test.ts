import { mockClient } from 'aws-sdk-client-mock';
import { GetCallerIdentityCommand, STSClient } from '@aws-sdk/client-sts';
import { beforeEach, describe, expect, it } from 'vitest';
import { checkAwsCredentials } from './identity.js';

const sts = mockClient(STSClient);

describe('checkAwsCredentials', () => {
  beforeEach(() => sts.reset());

  it('returns the caller identity', async () => {
    sts.on(GetCallerIdentityCommand).resolves({ Account: '123456789012', Arn: 'arn:aws:sts::123456789012:assumed-role/x/y' });
    expect(await checkAwsCredentials({ region: 'us-east-1', profile: 'sandbox' })).toEqual({ accountId: '123456789012', arn: 'arn:aws:sts::123456789012:assumed-role/x/y' });
  });

  it('fails when the credentials are for another account', async () => {
    sts.on(GetCallerIdentityCommand).resolves({ Account: '999999999999', Arn: 'a' });
    await expect(checkAwsCredentials({ region: 'us-east-1', expectedAccountId: '123456789012' })).rejects.toThrow(/expected sandbox account 123456789012/);
  });

  it('surfaces credential errors (e.g. an expired SSO session)', async () => {
    sts.on(GetCallerIdentityCommand).rejects(new Error('The SSO session associated with this profile has expired'));
    await expect(checkAwsCredentials({ region: 'us-east-1', profile: 'sso' })).rejects.toThrow(/SSO session/);
  });

  it('accepts an injected client', async () => {
    const client = { send: async () => ({ Account: '111111111111', Arn: 'arn' }) } as unknown as Pick<STSClient, 'send'>;
    expect((await checkAwsCredentials({ region: 'eu-west-1', client })).accountId).toBe('111111111111');
  });
});
