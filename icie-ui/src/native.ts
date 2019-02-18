import { spawn, ChildProcess } from "child_process";
import * as multijson from './multijson';

export interface QuickPickItem {
    label: string;
    description: string | null;
    detail: string | null;
    id: string;
}

export interface ReactionStatus {
    tag: "status";
    message: string | null;
}
export interface ReactionInfoMessage {
    tag: "info_message";
    message: string;
}
export interface ReactionErrorMessage {
    tag: "error_message";
    message: string;
}
export interface ReactionQuickPick {
    tag: "quick_pick";
    items: QuickPickItem[];
}
export interface ReactionInputBox {
    tag: "input_box";
    prompt: string | null;
    placeholder: string | null;
    password: boolean;
    ignoreFocusOut: boolean;
}
export interface ReactionConsoleLog {
    tag: "console_log";
    message: string;
}
export interface ReactionSaveAll {
    tag: "save_all";
}
export interface ReactionOpenFolder {
    tag: "open_folder";
    path: string;
    in_new_window: boolean;
}
export interface ReactionConsoleError {
    tag: "console_error";
    message: string;
}
export interface ReactionOpenEditor {
    tag: "open_editor";
    path: string;
    row: number;
    column: number;
}
export interface ReactionProgressStart {
    tag: "progress_start";
    id: string;
    title: string | null;
}
export interface ReactionProgressUpdate {
    tag: "progress_update";
    id: string;
    increment: number | null;
    message: string | null;
}
export interface ReactionProgressEnd {
    tag: "progress_end";
    id: string;
}

export interface ImpulseQuickPick {
    tag: "quick_pick";
    response: string | null;
}
export interface ImpulseInputBox {
    tag: "input_box";
    response: string | null;
}
export interface ImpulseTriggerBuild {
    tag: "trigger_build";
}
export interface ImpulseWorkspaceInfo {
    tag: "workspace_info";
    root_path: string | null;
}
export interface ImpulseTriggerTest {
    tag: "trigger_test";
}
export interface ImpulseSavedAll {
    tag: "saved_all";
}
export interface ImpulseTriggerInit {
    tag: "trigger_init";
}
export interface ImpulseTriggerSubmit {
    tag: "trigger_submit";
}
export interface ImpulseTriggerManualSubmit {
    tag: "trigger_manual_submit";
}
export interface ImpulseTriggerTemplateInstantiate {
    tag: "trigger_template_instantiate";
}

export type Reaction = ReactionStatus | ReactionInfoMessage | ReactionErrorMessage | ReactionQuickPick | ReactionInputBox | ReactionConsoleLog | ReactionSaveAll | ReactionOpenFolder | ReactionConsoleError | ReactionOpenEditor | ReactionProgressStart | ReactionProgressUpdate | ReactionProgressEnd;
export type Impulse = ImpulseQuickPick | ImpulseInputBox | ImpulseTriggerBuild | ImpulseWorkspaceInfo | ImpulseSavedAll | ImpulseTriggerTest | ImpulseTriggerInit | ImpulseTriggerSubmit | ImpulseTriggerManualSubmit | ImpulseTriggerTemplateInstantiate;

export class Logic {
    path: string;
    kid: ChildProcess;
    parser: multijson.Parser<Reaction>;
    constructor(extensionPath: string) {
        this.path = `${extensionPath}/assets/bin-linux`;
        this.kid = spawn(this.path, [], {});
        this.parser = new multijson.Parser<Reaction>();
    }
    send(impulse: Impulse) {
        console.log(`   ~> ${JSON.stringify(impulse)}`);
        this.kid.stdin.write(`${JSON.stringify(impulse)}\n`);
    }
    recv(callback: (reaction: Reaction) => void) {
        this.kid.stdout.on('data', chunk => {
            if (typeof chunk === 'string' || chunk instanceof String) {
                throw new Error('icie_stdio.stdout.on [data] returned string instead of Buffer');
            }
            this.parser.write(chunk);
            for (let reaction of this.parser.read()) {
                console.log(`<~    ${JSON.stringify(reaction)}`);
                callback(reaction);
            }
        });
    }
}